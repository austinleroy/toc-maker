pub struct Config {
    pub inpath: String,
    pub outpath: String,
    pub use_zlib: bool,
    pub hash_metadata: bool,
}

impl Config {
    pub fn new(mut args: std::env::Args) -> Result<Self, String> {
        args.next(); //Skip executable path

        let mut inpath = None;
        let mut outpath = None;
        #[allow(unused_mut)]
        let mut use_zlib = false;
        #[allow(unused_mut)]
        let mut hash_metadata = false;
        
        while let Some(arg) = args.next() {
            if !arg.starts_with('-') {
                if matches!(inpath, None) {
                    inpath = Some(arg);
                } else if matches!(outpath, None) {
                    outpath = Some(arg);
                } else {
                    return Err(format!("Unexpected argument: {arg}"));
                }
            } else {
                #[cfg(feature = "zlib")]
                if arg == "-z" || arg == "--zlib" {
                    use_zlib = true;
                    continue;
                }

                #[cfg(feature = "hash_meta")]
                if arg == "-m" || arg == "--meta" {
                    hash_metadata = true;
                    continue;
                }

                if arg == "-h" || arg == "--help" {
                    return Err(String::new());
                }

                return Err(format!("Unexpected argument: {arg}"));
            }
        }

        Ok(Self {
            inpath: inpath.ok_or("Must specify input path")?,
            outpath: outpath.ok_or("Must specify output path")?,
            use_zlib,
            hash_metadata,
        })
    }

    pub fn usage() -> &'static str {
        r#"

Creates a utoc, ucas, and pak file using files in the input directory. Built
and tested using UE4.27 (no guarantees on other verions).

Usage:     toc-maker [options] <input path> <output path>

    <input path>    Path to folder containing files that should be packaged 
                    into the IoStore output. Directory structure matters - this
                    folder will be considered the root of the output package.

    <output path>   Path to the desired output. Output will be used as the file
                    stem for newly created .utoc, .ucas, and .pak files.

    Options:

      -h, --help    Show this help and exit.

      -z, --zlib    Compress output data using zlib. Can substantially reduce 
                    package size when including textures/models.

      -m, --meta    Hash file contents and include in toc meta. Doesn't seem to
                    be verified, but may help if you have issues loading 
                    content. ***INCREASES EXECUTION TIME***

        "#
    }
}