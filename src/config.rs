pub struct Config {
    pub inpath: String,
    pub outpath: String,
    #[cfg(feature = "zlib")]
    pub use_zlib: bool,
}

impl Config {
    pub fn new(mut args: std::env::Args) -> Result<Self, String> {
        args.next(); //Skip executable path

        let mut inpath = None;
        let mut outpath = None;
        #[cfg(feature = "zlib")]
        let mut use_zlib = false;
        
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

                return Err(format!("Unexpected argument: {arg}"));
            }
        }

        Ok(Self {
            inpath: inpath.ok_or("Must specify input path")?,
            outpath: outpath.ok_or("Must specify output path")?,
            #[cfg(feature = "zlib")]
            use_zlib,
        })
    }

    pub fn usage() -> &'static str {
        r#"

Creates a utoc, ucas, and pak file using files in the input directory.  Developed and tested
for UE4.27 (no guarantees on other verions).

Usage:     toc-maker <input path> <output path>

    <input path>    Path to folder containing files that should be packaged into the IoStore
                    output. Directory structure matters - this folder will be considered the
                    root of the output package.

    <output path>   Path to the desired output.  Output will be used as the file stem for
                    newly created .utoc, .ucas, and .pak files.


        "#
    }
}