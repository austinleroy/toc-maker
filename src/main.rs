use std::fs::File;

mod asset_collector;
mod toc_factory;
mod io_package;
mod io_toc;
mod string;
mod metadata;
mod platform;
mod helpers;

use string::Hasher16;
use toc_factory::TocFactory;

fn main() {
    

    //println!("{:x}",Hasher16::get_cityhash64("P3R"));
}