use std::{fs::File, io::Write, iter};
use num::{PrimInt, Unsigned};


pub trait AlignableNum: PrimInt + Unsigned {
    fn align_to<T: Into<Self>>(&self, alignment_size: T) -> Self {
        let al = alignment_size.into();
        let next = *self + al - Self::one();
        next - (next % al)
    }   
}

impl AlignableNum for u8 {}
impl AlignableNum for u16 {}
impl AlignableNum for u32 {}
impl AlignableNum for u64 {}
impl AlignableNum for u128 {}

pub trait AlignableStream: Write {
    fn align_to<O: AlignableNum + TryInto<usize>, T: Unsigned + Into<O>>(&mut self, absolute_offset: &mut O, alignment_size: T) -> O {
        let next_alignment = absolute_offset.align_to(alignment_size);
        if next_alignment != *absolute_offset {
            match (next_alignment - *absolute_offset).try_into() {
                Ok(s) => {
                    let blank: Vec<u8> = iter::repeat(0).take(s).collect();
                    self.write(&blank).unwrap();
                }
                Err(_) => panic!("Oversized alignment difference!!")
            }
        }
        *absolute_offset = next_alignment;
        *absolute_offset
    }
}

impl AlignableStream for File {}