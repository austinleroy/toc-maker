use std::{
    any::Any, cell::RefCell, cmp::Ordering, error::Error, fs::{File, Metadata}, io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write}, iter, mem, ops::Deref, path::{Path, PathBuf}, pin::Pin, sync::{Arc, Mutex, MutexGuard, RwLock, Weak}, time::Instant
};
use byteorder::NativeEndian;

use crate::{
    asset_collector::{
        self, AssetCollector, TocDirectory, TocDirectorySyncRef, TocFile, TocFileSyncRef, GAME_ROOT, SUITABLE_FILE_EXTENSIONS}, helpers::{AlignableNum, AlignableStream}, io_package::{
        ContainerHeaderPackage,
        ExportBundle, ExportBundleHeader4,
        PackageIoSummaryDeserialize, 
        PackageSummary2}, io_toc::{
        ContainerHeader, IoChunkId, IoChunkType4, IoDirectoryIndexEntry, IoFileIndexEntry, IoOffsetAndLength, IoStoreTocCompressedBlockEntry, IoStoreTocEntryMeta, IoStoreTocHeaderCommon, IoStoreTocHeaderType3, IoStringPool, IO_FILE_INDEX_ENTRY_SERIALIZED_SIZE
    }, metadata::{UtocMetadata, UTOC_METADATA}, string::{FString32NoHash, FStringSerializer, FStringSerializerExpectedLength, Hasher16, Hasher8}
};

pub const DEFAULT_COMPRESSION_BLOCK_ALIGNMENT: u32 = 0x10;

struct TocFlattener {
    // Used to set the correct directory/file/string indices when flattening TocDirectory tree into Directory Index entries
    io_dir_entries: Vec<IoDirectoryIndexEntry>,
    io_file_entries: Vec<IoFileIndexEntry>,
    entry_names: Vec<String>,
}

impl TocFlattener {
    pub fn flatten(dir: TocDirectorySyncRef) -> (Vec<IoDirectoryIndexEntry>, Vec<IoFileIndexEntry>, Vec<String>) {
        let mut flattener = Self {
            io_dir_entries: vec![],
            io_file_entries: vec![],
            entry_names: vec![],
        };

        flattener.flatten_dir(dir);

        
        (flattener.io_dir_entries, flattener.io_file_entries, flattener.entry_names)
    }

    fn flatten_dir(&mut self, dir: TocDirectorySyncRef) {
        let mut io_dir_entry = IoDirectoryIndexEntry {
            name: match dir.read().unwrap().name.as_ref() {
                Some(t) => self.get_name_index(t),
                None => u32::MAX
            },
            first_child: u32::MAX,
            next_sibling: u32::MAX,
            first_file: u32::MAX,
        };

        // Files first
        if let Some(first_file) = dir.read().unwrap().first_file.clone() {
            io_dir_entry.first_file = self.io_file_entries.len() as u32;
            
            let dir_hash_path = {
                // travel upwards through parents to build hash path
                // calculate hash after validation so it's easier to remove incorrectly formatted uassets
                let mut path_comps: Vec<String> = vec![];
                let mut next_parent = Some(dir.clone());
                while let Some(curr_parent) = next_parent {
                    if let Some(t) = curr_parent.read().unwrap().name.as_ref() {
                        path_comps.insert(0, t.to_owned());
                    }
                    next_parent = curr_parent.read().unwrap().parent.upgrade();
                }
                path_comps.join("/") + "/"
            };

            let mut next_file = Some(first_file);
            while let Some(curr_file) = next_file {
                let curr_file = curr_file.read().unwrap();
                let flat_file = IoFileIndexEntry {
                    name: self.get_name_index(&curr_file.name),
                    next_file: if curr_file.next.is_some() { self.io_file_entries.len() as u32 + 1 } else { u32::MAX },
                    user_data: self.io_file_entries.len() as u32,
                    file_size: curr_file.file_size,
                    os_path: curr_file.os_file_path.clone(),
                    chunk_id: TocFlattener::get_file_hash(&dir_hash_path, curr_file.deref())
                };
                self.io_file_entries.push(flat_file);
                next_file = curr_file.next.clone();
            }
        }
        
        // Add this directory to the list
        let curr_dir_pos = self.io_dir_entries.len();
        self.io_dir_entries.push(io_dir_entry);

        // Then iterate subdirectories
        if let Some(first_child) = dir.read().unwrap().first_child.clone() {
            let first_child_index = self.io_dir_entries.len() as u32;
            let io_dir_entry = self.io_dir_entries.get_mut(curr_dir_pos).unwrap();
            io_dir_entry.first_child = first_child_index;
            self.flatten_dir(first_child);
        }

        // Then move on to the next sibling
        if let Some(next_sibling) = dir.read().unwrap().next_sibling.clone() {
            let next_sibling_index = self.io_dir_entries.len() as u32;
            let io_dir_entry = self.io_dir_entries.get_mut(curr_dir_pos).unwrap();
            io_dir_entry.next_sibling = next_sibling_index;
            self.flatten_dir(next_sibling);
        }

    }

    fn get_name_index(&mut self, test: &str) -> u32 {
        (match self.entry_names.iter().position(|name| name == test) {
            Some(i) => i,
            None => {
                self.entry_names.push(test.to_string());
                self.entry_names.len() - 1
            },
        }) as u32
    }

    fn get_file_hash(dir_path: &str, curr_file: &TocFile) -> IoChunkId {
        let (stem, extension) = curr_file.name.split_once('.').expect("Should always be a filename with an extension.");
        let chunk_type = if SUITABLE_FILE_EXTENSIONS.contains(&extension) {
            match extension {
                "uasset" | "umap" => IoChunkType4::ExportBundleData, //.uasset, .umap
                "ubulk" => IoChunkType4::BulkData, // .ubulk
                "uptnl" => IoChunkType4::OptionalBulkData, // .uptnl
                _ => panic!("CRITICAL ERROR: Did not get a supported file extension. This should've been handled earlier")
            }
        } else {
            // this file should've been skipped, see add_folder in asset_collector.rs
            panic!("CRITICAL ERROR: Did not get a supported file extension. This should've been handled earlier")
        };
        let mut dir_path = dir_path.to_string() + stem;
        if !dir_path.starts_with("Game") {
            dir_path = "Game/".to_string() + dir_path.split_once('/').unwrap().1;
        }
        let path_to_replace_split = dir_path.split_once("/Content").unwrap();
        let path_to_replace = "/".to_owned() + path_to_replace_split.0 + path_to_replace_split.1;
        IoChunkId::new(&path_to_replace, chunk_type)
    }
}

pub struct TocFactory {
    source_folder: String,
    max_compression_block_size: u32,
    compression_block_alignment: u32,
    //toc_name_hash: u64,
    // chunk_ids: Vec<IoChunkId>,
    // offsets_and_lengths: Vec<IoOffsetAndLength>,
    // compression_blocks: Vec<IoStoreTocCompressedBlockEntry>,
    // metas: Vec<IoStoreTocEntryMeta>,
    // cas_pointer: u64, // Current virtual position of container file


}

impl TocFactory {
    pub fn new(source_folder: &str) -> Self {
        Self { 
            source_folder: source_folder.to_string(),
            // Directory block
            max_compression_block_size: 0x40000, // default for UE 4.26/4.27 is 0x10000 - used for offset + length offset
            compression_block_alignment: DEFAULT_COMPRESSION_BLOCK_ALIGNMENT, // 0x800 is default for UE 4.27
            //toc_name_hash: Hasher16::get_cityhash64(GAME_ROOT), // used for container id (is also the last file in partition) (verified)
            // chunk_ids: vec![],
            // offsets_and_lengths: vec![],
            // compression_blocks: vec![],
            // metas: vec![],
            // cas_pointer: 0
        }
    }

    pub fn write_files<WTOC: Write, WCAS: AlignableStream>(self, mut utoc_stream: &mut WTOC, mut ucas_stream: &mut WCAS) -> Result<(), &'static str> {
        type EN = byteorder::NativeEndian;
        let asset_collector = AssetCollector::from_folder(&self.source_folder)?;
        let (
            directories,
            mut files,
            names
        ) = TocFlattener::flatten(asset_collector.get_toc_tree());

        //TODO move mount point and set toc_name_hash
        //TODO also remove meta hashes?  Since they don't seem to be needed
        //TODO remove?  Doesn't seem like this helps... was working on this for ubulks since they seem to have issues
        files.sort_by(|a,b| {
            let apar: String = a.os_path.split('/').rev().skip(1).collect();
            let bpar: String = b.os_path.split('/').rev().skip(1).collect();
            let pord = apar.cmp(&bpar);
            if a.os_path.ends_with(".ubulk") {
                if b.os_path.ends_with(".ubulk") {
                    if matches!(pord, Ordering::Equal) {
                        a.file_size.cmp(&b.file_size)
                    } else {
                        pord
                    }
                } else {
                    Ordering::Greater
                }
            } else if b.os_path.ends_with(".ubulk") {
                Ordering::Less
            } else {
                if matches!(pord, Ordering::Equal) {
                    a.file_size.cmp(&b.file_size)
                } else {
                    pord
                }
            }
        });
        for i in 0..files.len() {
            files[i].user_data = i as u32;
        }


        let toc_name_hash = Hasher16::get_cityhash64("pakchunk999"); // This can be anything - in UE4.27, this is the pakchunk number, e.g. pakchunk120
        let mount_point = "../../../";

        // CAS STUFF
        let mut container_header = ContainerHeader::new(toc_name_hash);
        let mut compression_blocks = vec![];
        let mut offsets_and_lengths = vec![];
        let mut metas = vec![];
        let mut uncompressed_offset = 0u64;
        let mut compressed_offset = 0u64;
        for file in files.iter() {
            // File offsets and lengths relates to uncompressed data
            uncompressed_offset = uncompressed_offset.align_to(self.max_compression_block_size);
            offsets_and_lengths.push(IoOffsetAndLength::new(uncompressed_offset, file.file_size));
            uncompressed_offset += file.file_size;

            // Compression splits the file into "max_compression_block_size" sized chunks and compresses them.
            // These compressed chunks are then written to the file one by one, with chunk start locations aligned to compression_block_alignment
            // This is what goes into the compression_blocks array - chunk start, then compressed size, then uncompressed size
            let mut compressed_chunks = self.write_compressed_file(&file, &mut compressed_offset, ucas_stream);
            compression_blocks.append(&mut compressed_chunks);

            if file.chunk_id.get_type() == IoChunkType4::ExportBundleData {
                let os_file = File::open(&file.os_path).unwrap(); // Export Bundles (.uasset) have store entry data written
                let mut file_reader = BufReader::with_capacity(Self::FILE_SUMMARY_READER_ALLOC, os_file);
                container_header.packages.push(ContainerHeaderPackage::from_package_summary::<
                    ExportBundleHeader4, PackageSummary2, BufReader<File>, EN
                >(
                    &mut file_reader, file.chunk_id.get_raw_hash(), 
                    file.file_size, &file.os_path
                ));
            }

            //self.metas.push(IoStoreTocEntryMeta::new_empty()); // Generate meta - SHA1 hash of the file's contents (doesn't seem to be required)
            metas.push(IoStoreTocEntryMeta::new_with_hash(&mut File::open(Path::new(&file.os_path)).unwrap()));
        }

        //Container header is last thing to write to file
        let container_header = container_header.to_buffer::<WCAS, EN>(&mut ucas_stream).unwrap(); // write our container header in the buffer
        offsets_and_lengths.push(IoOffsetAndLength::new(uncompressed_offset.align_to(self.max_compression_block_size), container_header.len() as u64));
        ucas_stream.align_to(&mut compressed_offset, self.max_compression_block_size);
        ucas_stream.write(&container_header);
        compression_blocks.push(IoStoreTocCompressedBlockEntry::new(compressed_offset, container_header.len() as u32, container_header.len() as u32));

        //self.metas.push(IoStoreTocEntryMeta::new_empty());
        metas.push(IoStoreTocEntryMeta::new_with_hash(&mut Cursor::new(container_header)));



        // TOC STUFF
        // Get DirectoryIndexSize = mount point + Directory Entries + File Entries + Strings
        // Each section contains a u32 to note the object count
        let mount_point_bytes = (mem::size_of::<u32>() + mount_point.len() + 1) as u32;
        let directory_index_bytes = (directories.len() * std::mem::size_of::<IoDirectoryIndexEntry>() + mem::size_of::<u32>()) as u32;
        let file_index_bytes = (files.len() * IO_FILE_INDEX_ENTRY_SERIALIZED_SIZE + mem::size_of::<u32>()) as u32;
        let mut string_index_bytes = mem::size_of::<u32>() as u32;
        names.iter().for_each(|name| string_index_bytes += FString32NoHash::get_expected_length(name) as u32);
        let directory_index_size = mount_point_bytes + directory_index_bytes + file_index_bytes + string_index_bytes;

        let toc_header = IoStoreTocHeaderType3::new(
            toc_name_hash, 
            files.len() as u32 + 1, // + 1 for container header
            compression_blocks.len() as u32,
            self.max_compression_block_size,
            directory_index_size
        );
        // FIoStoreTocHeader
        toc_header.to_buffer::                          <WTOC, EN>(&mut utoc_stream).unwrap(); // FIoStoreTocHeader
        IoChunkId::list_to_buffer::                     <WTOC, EN>(&files.iter().map(|f| f.chunk_id).chain([IoChunkId::new_from_hash(toc_name_hash, IoChunkType4::ContainerHeader)]).collect(), &mut utoc_stream).unwrap(); // FIoChunkId
        IoOffsetAndLength::list_to_buffer::             <WTOC, EN>(&offsets_and_lengths, &mut utoc_stream).unwrap(); // FIoOffsetAndLength
        IoStoreTocCompressedBlockEntry::list_to_buffer::<WTOC, EN>(&compression_blocks, &mut utoc_stream).unwrap(); // FIoStoreTocCompressedBlockEntry
        // compression methods go here if we want to do any compressing
        FString32NoHash::to_buffer::                    <WTOC, EN>(mount_point, &mut utoc_stream).unwrap(); // Mount Point
        IoDirectoryIndexEntry::list_to_buffer::         <WTOC, EN>(&directories, &mut utoc_stream).unwrap(); // FIoDirectoryIndexEntry
        IoFileIndexEntry::list_to_buffer::              <WTOC, EN>(&files, &mut utoc_stream).unwrap(); // FIoFileIndexEntry
        IoStringPool::list_to_buffer::                  <WTOC, EN>(&names, &mut utoc_stream).unwrap(); // FIoStringIndexEntry
        IoStoreTocEntryMeta::list_to_buffer::           <WTOC, EN>(&metas, &mut utoc_stream).unwrap(); // FIoStoreTocEntryMeta

        Ok(())
    }

    fn write_compressed_file<W: AlignableStream>(&self, file: &IoFileIndexEntry, offset: &mut u64, destination: &mut W) -> Vec<IoStoreTocCompressedBlockEntry> {
        let compression_block_count = (file.file_size / self.max_compression_block_size as u64) + 1; // need at least 1 compression block
        let mut gen_blocks = Vec::with_capacity(compression_block_count as usize);

        let mut reader = File::open(&file.os_path).unwrap();
        let mut data = vec![0u8; self.max_compression_block_size as usize];
        while let Ok(len) = reader.read(&mut data) {
            if len == 0 { break }

            //TODO allow compressing data?
            // data = file.compression_func(data)
            // compressed_len = compressed_len
            let compressed_len = len;

            destination.align_to(offset, self.compression_block_alignment);
            gen_blocks.push(IoStoreTocCompressedBlockEntry::new(*offset, len as u32, compressed_len as u32));
            *offset += destination.write(&data[..len]).unwrap() as u64;
        }

        gen_blocks
    }

    pub const FILE_SUMMARY_READER_ALLOC: usize = 0x2000;
}

// TODO: Set the mount point further up in mods where the file structure doesn't diverge at root


pub struct ContainerData {
    pub header: Vec<u8>,
    pub virtual_blocks: Vec<PartitionBlock>,
    pub files: Vec<String>
}

#[repr(C)]
pub struct PartitionBlock {
    //os_path: *const u8, // 0x0
    pub os_path: usize, // 0x0 (pointers don't implement Send or Sync)
    pub start: u64, // 0x8
    pub length: u64, // 0x10
}

pub struct TocBuilderProfiler {
    // All file sizes are in bytes
    successful_files: u64,
    successful_files_size: u64,
    incorrect_asset_format: Vec<String>, // list of offending files, print out to console
    incorrect_asset_format_size: u64,
    failed_to_read: Vec<String>,
    failed_to_read_size: u64,
    container_header_hash: u64,
    compression_block_count: u64,
    mount_point: String,
    directory_index_size: u64,
    file_index_size: u64,
    string_index_size: u64,
    generated_meta_hashes: bool,
    start_time: Instant,
    time_to_flatten: u128,
    time_to_serialize: u128
}

impl TocBuilderProfiler {
    pub fn new() -> Self {
        Self {
            successful_files: 0,
            successful_files_size: 0,
            incorrect_asset_format: vec![],
            incorrect_asset_format_size: 0,
            failed_to_read: vec![],
            failed_to_read_size: 0,
            container_header_hash: 0,
            compression_block_count: 0,
            mount_point: String::new(),
            directory_index_size: 0,
            file_index_size: 0,
            string_index_size: 0,
            generated_meta_hashes: false,
            start_time: Instant::now(),
            time_to_flatten: 0,
            time_to_serialize: 0
        }
    }

    fn set_flatten_time(&mut self) {
        self.time_to_flatten = self.start_time.elapsed().as_micros();
    }
    fn set_serialize_time(&mut self) {
        self.time_to_serialize = self.start_time.elapsed().as_micros();
    }
    fn display_results(&self) {
        // TODO: Advanced display results
        println!("Flatten Time: {} ms", self.time_to_flatten as f64 / 1000f64);
        println!("Serialize Time: {} ms", self.time_to_serialize as f64 / 1000f64);
    }
}