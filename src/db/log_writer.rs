use crate::db::log_format::{
    kBlockSize, kHeaderSize, kMaxRecordType, kRecyclableHeaderSize, RecordType,
};
use crate::env::WritableFile;
use crate::util::coding::{encode_fixed32, encode_fixed64};
use crate::util::file_reader_writer::WritableFileWriter;
use crate::util::hash::crc32;
use crate::util::status::State;

#[derive(Debug)]
pub struct Writer<T: WritableFile> {
    dest_: WritableFileWriter<T>,
    block_offset_: usize, // Current offset in block
    log_number_: u64,
    recycle_log_files_: bool,
    manual_flush_: bool,
    type_crc_: Vec<u32>,
}

impl<T: WritableFile> Drop for Writer<T> {
    fn drop(&mut self) {
        self.dest_.close();
    }
}

impl<T: WritableFile> Writer<T> {
    pub fn new(
        dest: WritableFileWriter<T>,
        log_number: u64,
        recycle_log_files: bool,
        manual_flush: bool,
    ) -> Writer<T> {
        let mut type_crc: [u32; kMaxRecordType as usize + 1] = [0u32; kMaxRecordType as usize + 1];
        for x in 0..kMaxRecordType + 1 {
            type_crc[x as usize] = crc32(0, &[x]);
        }
        Writer {
            dest_: dest,
            block_offset_: 0,
            log_number_: log_number,
            recycle_log_files_: recycle_log_files,
            manual_flush_: manual_flush,
            type_crc_: type_crc.to_vec(),
        }
    }
    /*const Slice& slice*/
    pub fn add_record(&mut self, slice: Vec<u8>) {
        /*
        const char* ptr = slice.data();
        size_t left = slice.size();
        */
        let mut ptr = slice.as_slice();
        let mut left = slice.len();
        let header_size = if self.recycle_log_files_ {
            kRecyclableHeaderSize
        } else {
            kHeaderSize
        };
        let mut begin = true;
        loop {
            let fragment_length: usize;
            let leftover: usize = kBlockSize - self.block_offset_;
            assert!(leftover >= 0);

            if leftover < header_size {
                if leftover > 0 {
                    assert!(header_size <= 11);
                    let s = self.dest_.append(
                        vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                            [..leftover]
                            .to_vec(),
                    );
                    if !s.is_ok() {
                        break;
                    }
                }
                self.block_offset_ = 0;
            }
            assert!((kBlockSize - self.block_offset_) >= header_size);
            let avail: usize = kBlockSize - self.block_offset_ - header_size;
            fragment_length = if left < avail { left } else { avail };

            let end: bool = left == fragment_length;
            let rtype: RecordType;
            if begin && end {
                rtype = if self.recycle_log_files_ {
                    RecordType::kRecyclableFullType
                } else {
                    RecordType::kFullType
                };
            } else if begin {
                rtype = if self.recycle_log_files_ {
                    RecordType::kRecyclableFirstType
                } else {
                    RecordType::kFirstType
                };
            } else if end {
                rtype = if self.recycle_log_files_ {
                    RecordType::kRecyclableLastType
                } else {
                    RecordType::kLastType
                };
            } else {
                rtype = if self.recycle_log_files_ {
                    RecordType::kRecyclableMiddleType
                } else {
                    RecordType::kMiddleType
                };
            };
            let s = self.emit_physical_record(rtype, ptr.to_vec(), fragment_length);
            ptr = &ptr[fragment_length..];
            left -= fragment_length;
            begin = false;

            if !(s.is_ok() && left > 0) {
                break;
            }
        }
    }

    fn emit_physical_record(&mut self, t: RecordType, ptr: Vec<u8>, n: usize) -> State {
        let mut header_size: usize = 0;
        let mut buf: [u8; kRecyclableHeaderSize] = [0u8; kRecyclableHeaderSize];
        let mut crc = self.type_crc_[t as usize];

        buf[4] = (n & 0xffusize) as u8;
        buf[5] = (n >> 8) as u8;
        buf[6] = t as u8;

        if (t as u8) < RecordType::kRecyclableFullType as u8 {
            header_size = kHeaderSize;
        } else {
            header_size = kRecyclableHeaderSize;
            let lnSlice = encode_fixed64(self.log_number_);
            buf[7] = lnSlice[0];
            buf[8] = lnSlice[1];
            buf[9] = lnSlice[2];
            buf[10] = lnSlice[3];
            crc = crc32(crc, &buf[4..kRecyclableHeaderSize]);
        }
        crc = crc32(crc, &ptr.as_slice());
        buf[..4].clone_from_slice(&encode_fixed32(crc));

        let mut s = self.dest_.append(buf[..header_size].to_vec());
        if s.is_ok() {
            s = self.dest_.append(ptr);
            if s.is_ok() {
                if self.manual_flush_ {
                    s = self.dest_.flush()
                }
            }
        }
        self.block_offset_ += header_size + n;
        return s;
    }
}
