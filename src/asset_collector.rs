use std::{
    fs::{self, File}, 
    io::BufReader, 
    path::{Path, PathBuf},
    sync::{Arc, RwLock, Weak}
};

use crate::io_package;
use crate::platform::Metadata;

pub type TocDirectorySyncRef = Arc<RwLock<TocDirectory>>;
pub type TocFileSyncRef = Arc<RwLock<TocFile>>;

pub const SUITABLE_FILE_EXTENSIONS: &'static [&'static str] = ["uasset", "ubulk", "uptnl", "umap"].as_slice();

pub struct AssetCollector
{
    root_dir: TocDirectorySyncRef,
    profiler: AssetCollectorProfiler,
}

impl AssetCollector
{
    pub fn from_folder(path: &str) -> Result<Self, &'static str> {
        if Path::exists(Path::new(&path)) {
            let root_dir = TocDirectory::new_rc(None);
            let mut profiler = AssetCollectorProfiler::new(path.to_string());
            
            let path: PathBuf = PathBuf::from(path);
            AssetCollector::add_folder(&path, &root_dir, &mut profiler);
            Ok(Self {
                root_dir,
                profiler,
            })
        } else {
            Err("AssetCollector->from_folder: Path does not exist")
        }
    }

    pub fn get_toc_tree(self) -> TocDirectorySyncRef {
        self.root_dir
    }

    pub fn print_stats(&self) {
        self.profiler.print();
    }

    fn add_folder(os_folder_path: &PathBuf, toc_folder_path: &TocDirectorySyncRef, mut profiler: &mut AssetCollectorProfiler) {
        for file_entry in fs::read_dir(os_folder_path).unwrap() {
            match &file_entry {
                Ok(fs_obj) => {
                    let name = fs_obj.file_name().into_string().unwrap(); 
                    let file_type = fs_obj.file_type().unwrap();
                    if file_type.is_dir() {
                        let mut inner_path = PathBuf::from(os_folder_path);
                        inner_path.push(&name);
                        let mut new_dir = TocDirectory::new_rc(Some(name));
                        toc_folder_path.add_directory(new_dir.clone());
                        AssetCollector::add_folder(&inner_path,&mut new_dir, &mut profiler);
                        profiler.add_directory();
                    } else if file_type.is_file() {
                        let file_size = Metadata::get_object_size(fs_obj);
                        match PathBuf::from(&name).extension().map(|e| e.to_str().unwrap()) {
                            Some(file_extension) => {
                                if SUITABLE_FILE_EXTENSIONS.contains(&file_extension) {
                                    if file_extension == "uasset" || file_extension == "umap" { // export bundles - requires checking file header to ensure that it doesn't have the cooked asset signature
                                        let current_file = File::open(fs_obj.path()).unwrap();
                                        let mut file_reader = BufReader::with_capacity(4, current_file);
                                        if !io_package::is_valid_asset_type::<BufReader<File>, byteorder::NativeEndian>(&mut file_reader) {
                                            profiler.add_skipped_file(os_folder_path.to_str().unwrap(), format!("Was not in TOC-specific uasset format"), file_size);
                                            println!("{name} skipped");
                                            continue;
                                        }
                                    }
                                    let new_file = TocFile::new_rc(&name, file_size, fs_obj.path().to_str().unwrap());
                                    toc_folder_path.write().unwrap().add_file(new_file);
                                    profiler.add_added_file(file_size);
                                } else {
                                    profiler.add_skipped_file(fs_obj.path().to_str().unwrap(), format!("Unsupported file type"), file_size);
                                }
                            },
                            None => {
                                profiler.add_skipped_file(fs_obj.path().to_str().unwrap(), format!("No file extension"), file_size);
                            }
                        }
                    }
                },
                Err(e) => profiler.add_failed_fs_object(os_folder_path.to_str().unwrap(), e.to_string())
            }
        }
    }
}

// Create tree of assets that can be used to build a TOC

//      A <--------
//      ^    ^    ^
//      |    |    | (refs from child -> parent)
//      v    |    | (owns from parent -> child and in sibling and file linked lists)
//      B -> C -> D

pub struct TocDirectory {
    pub name:           Option<String>, // leaf name only (directory name or file name)
    pub parent:         Weak        <RwLock<TocDirectory>>, // weakref to parent for path building for FIoChunkIds
    pub first_child:    Option      <TocDirectorySyncRef>, // first child
    last_child:         Weak        <RwLock<TocDirectory>>, // O(1) insertion on directory add
    pub next_sibling:   Option      <TocDirectorySyncRef>, // next sibling
    pub first_file:     Option      <TocFileSyncRef>, // begin file linked list, owns file children
    last_file:          Weak        <RwLock<TocFile>>, // O(1) insertion on file add
}

impl TocDirectory {
    pub fn new(name: Option<String>) -> Self {
        Self {
            name,
            parent: Weak::new(),
            first_child: None,
            last_child: Weak::new(),
            next_sibling: None,
            first_file: None,
            last_file: Weak::new(),
        }
    }
    #[inline] // convenience function to create reference counted toc directories
    pub fn new_rc(name: Option<String>) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(TocDirectory::new(name)))
    }
    #[inline]
    pub fn has_children(&self) -> bool {
        match self.first_child {
            Some(_) => true,
            None => false
        }
    }
    #[inline]
    pub fn has_files(&self) -> bool {
        match self.first_file {
            Some(_) => true,
            None => false
        }
    }
    // Add a file child into directory that doesn't currently contain any other files
    fn add_file(&mut self, file: TocFileSyncRef) {
        if self.has_files() {
            self.last_file.upgrade().expect("Unable to upgrade last_file of dir, even though it has children!")
                .write().unwrap().add_sibling(file.clone());
        } else {
            self.first_file = Some(file.clone());
        }
        self.last_file = Arc::downgrade(&file);
    }
}

trait TocDir {
    fn add_directory(&self, dir: TocDirectorySyncRef);
}

impl TocDir for Arc<RwLock<TocDirectory>> {
    fn add_directory(&self, dir: TocDirectorySyncRef) {
        dir.write().unwrap().parent = Arc::downgrade(&self); // set child node's parent as weak ref of parent 
        let mut me = self.write().unwrap();
        if me.has_children() { 
            let last_child = me.last_child.upgrade().expect("Unable to upgrade last_child of dir, even though it has children!");
            assert!(last_child.read().unwrap().next_sibling.is_none(), "Sibling directory already set on last child of {}", me.name.as_deref().unwrap_or("root"));
            last_child.write().unwrap().next_sibling = Some(dir.clone());
        } else {
            me.first_child = Some(dir.clone());
        }
        me.last_child = Arc::downgrade(&dir);
    }
}

#[derive(Debug)]
pub struct TocFile {
    pub next: Option<TocFileSyncRef>,
    pub name: String,
    pub file_size: u64,
    pub os_file_path: String,
}

impl TocFile {
    fn new(name: &str, file_size: u64, os_path: &str) -> Self {
        Self {
            next: None,
            name: String::from(name),
            file_size,
            os_file_path: String::from(os_path)
        }
    }
    #[inline] // convenience function to create reference counted toc files
    pub fn new_rc(name: &str, file_size: u64, os_path: &str) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(TocFile::new(name, file_size, os_path)))
    }

    pub fn add_sibling(&mut self, sibling: TocFileSyncRef) {
        assert!(self.next.is_none(), "Calling 'add_sibling' on TocFile that already has one!");
        self.next = Some(sibling)
    }
}

#[derive(Debug, PartialEq)]
struct AssetCollectorProfilerFailedFsObject {
    os_path: String,
    reason: String
}

#[derive(Debug, PartialEq)]
struct AssetCollectorSkippedFileEntry {
    os_path: String,
    reason: String,
}

#[derive(Debug, PartialEq)]
struct AssetCollectorProfiler {
    os_path: String,
    failed_file_system_objects: Vec<AssetCollectorProfilerFailedFsObject>,
    directory_count: u64,
    added_files_count: u64,
    added_files_size: u64,
    replaced_files_count: u64,
    replaced_files_size: u64,
    skipped_files: Vec<AssetCollectorSkippedFileEntry>,
    skipped_file_size: u64,
}

impl AssetCollectorProfiler {
    pub fn new(root_path: String) -> Self {
        Self {
            os_path: root_path,
            failed_file_system_objects: vec![],
            directory_count: 0,
            added_files_size: 0,
            added_files_count: 0,
            replaced_files_count: 0,
            replaced_files_size: 0,
            skipped_files: vec![],
            skipped_file_size: 0,
        }
    }

    fn get_terminal_length() -> usize {
        80
    }

    pub fn print(&self) {
        println!("{}", "#".repeat(AssetCollectorProfiler::get_terminal_length()));
        println!("{}", self.os_path);
        println!("{}", "=".repeat(AssetCollectorProfiler::get_terminal_length()));
        println!("{} directories added", self.directory_count);
        println!("{} added files ({} KB)", self.added_files_count, self.added_files_size / 1024);
        println!("{} replaced files ({} KB)", self.replaced_files_count, self.replaced_files_size / 1024);
        if self.skipped_files.len() > 0 {
            println!("{}", "-".repeat(AssetCollectorProfiler::get_terminal_length()));
            println!("SKIPPED: {} FILES", self.skipped_files.len());
            for i in &self.skipped_files {
                println!("File: {}, reason: {}", i.os_path, i.reason);
            }
        }
        if self.failed_file_system_objects.len() > 0 {
            println!("{}", "-".repeat(AssetCollectorProfiler::get_terminal_length()));
            println!("FAILED TO LOAD: {} FILES", self.failed_file_system_objects.len());
            for i in &self.failed_file_system_objects {
                println!("Inside folder \"{}\", reason \"{}\"", i.os_path, i.reason);
            }
        }
        println!("{}", "=".repeat(AssetCollectorProfiler::get_terminal_length()));
    }

    pub fn add_failed_fs_object(&mut self, parent_dir: &str, reason: String) {
        self.failed_file_system_objects.push(AssetCollectorProfilerFailedFsObject { os_path: parent_dir.to_owned(), reason })
    }

    pub fn add_skipped_file(&mut self, os_path: &str, reason: String, size: u64) {
        self.skipped_files.push(AssetCollectorSkippedFileEntry { os_path: os_path.to_owned(), reason });
        self.skipped_file_size += size;
    }
    pub fn add_directory(&mut self) {
        self.directory_count += 1;
    }
    pub fn add_added_file(&mut self, size: u64) {
        self.added_files_count += 1;
        self.added_files_size += size;
    }
}