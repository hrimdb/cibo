
use crate::env::EnvOptions;
use crate::env::{SequentialFile, WritableFile};
use crate::util::aligned_buffer::truncate_to_page_boundary;
use crate::util::aligned_buffer::AlignedBuffer;
use crate::util::status::Code;
use crate::util::status::State;
use std::cmp::min;
use std::sync::atomic::AtomicIsize;

#[derive(Debug)]
pub struct WritableFileWriter<T: WritableFile> {
    writable_file_: T,
    filesize_: usize,
    max_buffer_size_: usize,
    pending_sync_: bool,
    buf_: AlignedBuffer,
    bytes_per_sync_: usize,
    last_sync_size_: usize,
    #[cfg(not(feature = "CIBO_LITE"))]
    next_write_offset_: usize,
    // uint64_t                bytes_per_sync_;
    // RateLimiter*            rate_limiter_;
    // Statistics* stats_;
}

impl<T: WritableFile> WritableFileWriter<T> {
    pub fn new(writable_file: T, options: EnvOptions) -> WritableFileWriter<T> {
        let mut buf: AlignedBuffer = Default::default();
        buf.alignment(4 * 1024);
        buf.allocate_new_buffer(65536, false);
        WritableFileWriter {
            writable_file_: writable_file,
            filesize_: 0,
            max_buffer_size_: options.writable_file_max_buffer_size,
            pending_sync_: false,
            bytes_per_sync_: options.bytes_per_sync,
            buf_: buf,
            last_sync_size_: 0,
            #[cfg(not(feature = "CIBO_LITE"))]
            next_write_offset_: 0,
        }
    }

    pub fn append(&mut self, slice: Vec<u8>) -> State {
        let mut s: State = State::ok();
        let mut src = 0;
        let _ptr = slice.as_slice();
        let mut left = slice.len();
        self.pending_sync_ = true;
        {
            let fsize = self.get_file_size();
            self.writable_file_.prepare_write(fsize, left);
        }
        if self.buf_.get_capacity() - self.buf_.get_current_size() < left {
            let mut cap = self.buf_.get_capacity();
            while cap < self.max_buffer_size_ {
                // See whether the next available size is large enough.
                // Buffer will never be increased to more than max_buffer_size_.
                let desired_capacity = min(cap * 2, self.max_buffer_size_);
                if desired_capacity - self.buf_.get_current_size() >= left
                    || (self.writable_file_.use_direct_io()
                        && desired_capacity == self.max_buffer_size_)
                {
                    self.buf_.allocate_new_buffer(desired_capacity, true);
                    break;
                }
                cap *= 2;
            }
        }
        // Flush only when buffered I/O
        if !self.writable_file_.use_direct_io()
            && (self.buf_.get_capacity() - self.buf_.get_current_size() < left)
        {
            let s: State;
            if self.buf_.get_current_size() > 0 {
                s = self.flush();
                if !s.is_ok() {
                    return s;
                }
            }
            assert!(self.buf_.get_current_size() == 0);
        }

        // We never write directly to disk with direct I/O on.
        // or we simply use it for its original purpose to accumulate many small
        // chunks
        if self.writable_file_.use_direct_io() || self.buf_.get_capacity() >= left {
            while left > 0 {
                let appended = self.buf_.append(slice[src..].to_vec(), left);
                left -= appended;
                src += appended;
                if left > 0 {
                    s = self.flush();
                    if !s.is_ok() {
                        break;
                    }
                }
            }
        } else {
            assert!(self.buf_.get_current_size() == 0);
            s = self.write_buffered(slice[src..].to_vec(), left);
        }

        if s.is_ok() {
            self.filesize_ += slice.len();
        }
        State::ok()
    }

    fn get_file_size(&self) -> usize {
        return self.filesize_;
    }

    pub fn flush(&mut self) -> State {
        let mut s: State = State::new(Code::KCorruption, String::from(""), String::from(""));
        if self.buf_.get_current_size() > 0 {
            if self.writable_file_.use_direct_io() {
                if cfg!(not(feature = "CIBO_LITE")) {
                    s = self.write_direct();
                }
            } else {
                let buf_len = self.buf_.get_current_size();
                let buf_start = self.buf_.buffer_start();
                let read_result = self.buf_.read(buf_start, buf_len);
                s = self.write_buffered(read_result, buf_len);
            }
            if !s.is_ok() {
                return s;
            }
        }
        s = self.writable_file_.flush();
        if !s.is_ok() {
            return s;
        }

        // sync OS cache to disk for every bytes_per_sync_
        // TODO: give log file and sst file different options (log
        // files could be potentially cached in OS for their whole
        // life time, thus we might not want to flush at all).

        // We try to avoid sync to the last 1MB of data. For two reasons:
        // (1) avoid rewrite the same page that is modified later.
        // (2) for older version of OS, write can block while writing out
        //     the page.
        // Xfs does neighbor page flushing outside of the specified ranges. We
        // need to make sure sync range is far from the write offset.
        if !self.writable_file_.use_direct_io() && self.bytes_per_sync_ > 0 {
            let k_bytes_not_sync_range: usize = 1024 * 1024;
            let k_bytes_align_when_sync: usize = 4 * 1024;
            if self.filesize_ > k_bytes_not_sync_range {
                let mut offset_sync_to = self.filesize_ - k_bytes_not_sync_range;
                offset_sync_to -= offset_sync_to % k_bytes_align_when_sync;
                assert!(offset_sync_to >= self.last_sync_size_);
                if offset_sync_to > 0
                    && offset_sync_to - self.last_sync_size_ >= self.bytes_per_sync_
                {
                    let last_sync_size_ = self.last_sync_size_;
                    s = self.range_sync(
                        last_sync_size_ as i64,
                        (offset_sync_to - last_sync_size_) as i64,
                    );
                    self.last_sync_size_ = offset_sync_to;
                }
            }
        }
        s
    }

    fn range_sync(&mut self, offset: i64, nbytes: i64) -> State {
        return self.writable_file_.range_sync(offset, nbytes);
    }

    fn write_buffered(&mut self, data: Vec<u8>, size: usize) -> State {
        let mut s: State;
        assert!(self.writable_file_.use_direct_io());
        let mut src = 0;
        let mut left = size;
        while left > 0 {
            let allowed;

            // if (rate_limiter_ != nullptr) {
            // allowed = rate_limiter_->RequestToken(
            // left, 0 /* alignment */, writable_file_->GetIOPriority(), stats_,
            // RateLimiter::OpType::kWrite);
            // } else {
            allowed = left;
            // }
            s = self.writable_file_.append(data[src..src + left].to_vec());
            if !s.is_ok() {
                return s;
            }

            left -= allowed;
            src += allowed;
        }
        self.buf_.size(0);
        State::ok()
    }

    pub fn close(&mut self) -> State {
        let mut s: State = State::ok();
        if !self.writable_file_.fcntl() {
            s = State::new(
                Code::KIOError,
                "writeable_file_ has closed".to_string(),
                "".to_string(),
            );
            return s;
        }

        //s = self.flush();
        let mut interim: State;
        if self.writable_file_.use_direct_io() {
            interim = self.writable_file_.truncate(self.filesize_);
            if interim.is_ok() {
                interim = self.writable_file_.sync();
            }
            if !interim.is_ok() && s.is_ok() {
                s = interim;
            }
        }
        s
    }

    #[cfg(not(feature = "CIBO_LITE"))]
    fn write_direct(&mut self) -> State {
        assert!(self.writable_file_.use_direct_io());
        let mut s: State = State::ok();
        let alignment: usize = self.buf_.get_alignment();
        assert!((self.next_write_offset_ % alignment) == 0);
        let file_advance = truncate_to_page_boundary(alignment, self.buf_.get_current_size());

        let leftover_tail = self.buf_.get_current_size() - file_advance;
        self.buf_.pad_to_aligment_with(0);

        let src = self.buf_.buffer_start();
        let mut write_offset = self.next_write_offset_;
        let mut left = self.buf_.get_current_size();
        unsafe {
            while left > 0 {
                //rate_limiter
                let size = left;
                let mut write_context = vec![0; size];
                write_context = self.buf_.read(src, size);

                s = self
                    .writable_file_
                    .positioned_append(write_context, write_offset);
                if !s.is_ok() {
                    self.buf_.size(file_advance + leftover_tail);
                    return s;
                }
                left -= size;

                let _src = src.offset(size as isize);

                write_offset += size;
                assert!((self.next_write_offset_ % alignment) == 0);
            }
        }
        if s.is_ok() {
            self.buf_.refit_tail(file_advance, leftover_tail);
            self.next_write_offset_ = file_advance;
        }
        s
    }
}

impl<T: WritableFile> Drop for WritableFileWriter<T> {
    fn drop(&mut self) {}
}

#[derive(Debug)]
pub struct SequentialFileReader<T: SequentialFile> {
    file_: T,
    offset_: AtomicIsize,
    // uint64_t                bytes_per_sync_;
    // RateLimiter*            rate_limiter_;
    // Statistics* stats_;
}

impl<T: SequentialFile> SequentialFileReader<T> {
    pub fn new(file: T) -> SequentialFileReader<T> {
        SequentialFileReader {
            file_: file,
            offset_: AtomicIsize::new(0),
        }
    }

    pub fn skip(&self, n: i64) -> State {
        self.file_.skip(n)
    }

    pub fn read(&mut self, n: usize, result: &mut Vec<u8>, scratch: &mut Vec<u8>) -> State {
        self.file_.read(n, result, scratch)
    }
}
