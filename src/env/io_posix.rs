use crate::env;
use crate::env::k_default_page_size;
use crate::env::{SequentialFile, WritableFile};
use crate::util::status::{Code, State};
use libc::c_int;
use std::ffi::CString;
use std::os::raw::c_char;
use std::usize;

pub fn clearerr(stream: *mut libc::FILE) {
    extern "C" {
        fn clearerr(stream: *mut libc::FILE);
    }
    unsafe {
        clearerr(stream);
    }
}

#[cfg(any(target_os = "macos"))]
unsafe fn posix_fread_unlocked(
    ptr: *mut libc::c_void,
    size: libc::size_t,
    nobj: libc::size_t,
    stream: *mut libc::FILE,
) -> libc::size_t {
    return libc::fread(ptr, size, nobj, stream);
}

#[cfg(any(target_os = "linux"))]
unsafe fn posix_fread_unlocked(
    ptr: *mut libc::c_void,
    size: libc::size_t,
    nobj: libc::size_t,
    stream: *mut libc::FILE,
) -> libc::size_t {
    return libc::fread_unlocked(ptr, size, nobj, stream);
}

fn SetFD_CLOEXEC(fd: i32, options: env::EnvOptions) {
    if options.set_fd_cloexec && fd > 0 {
        unsafe {
            libc::fcntl(
                fd,
                libc::F_SETFD,
                libc::fcntl(fd, libc::F_GETFD) | libc::FD_CLOEXEC,
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
unsafe fn errno_location() -> *const c_int {
    extern "C" {
        fn __error() -> *const c_int;
    }
    __error()
}

#[cfg(target_os = "bitrig")]
fn errno_location() -> *const c_int {
    extern "C" {
        fn __errno() -> *const c_int;
    }
    unsafe { __errno() }
}

#[cfg(target_os = "dragonfly")]
unsafe fn errno_location() -> *const c_int {
    extern "C" {
        fn __dfly_error() -> *const c_int;
    }
    __dfly_error()
}

#[cfg(target_os = "openbsd")]
unsafe fn errno_location() -> *const c_int {
    extern "C" {
        fn __errno() -> *const c_int;
    }
    __errno()
}

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn errno_location() -> *const c_int {
    extern "C" {
        fn __errno_location() -> *const c_int;
    }
    __errno_location()
}

#[derive(Debug)]
pub struct PosixWritableFile {
    filename_: String,
    use_direct_io_: bool,
    fd_: i32,
    preallocation_block_size_: usize,
    last_preallocated_block_: usize,
    filesize_: usize,
    logical_sector_size_: usize,
}

#[cfg(target_os = "macos")]
fn get_flag() -> i32 {
    libc::O_CREAT
}

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
fn get_flag() -> i32 {
    libc::O_CREAT | libc::O_DIRECT
}

fn get_logical_buffer_size() -> usize {
    if cfg!(not(target_os = "linux")) {
        return k_default_page_size;
    } else {
        return k_default_page_size;
        //Todo: support linux
    }
}

fn IsSectorAligned(off: usize, sector_size: usize) -> bool {
    return off % sector_size == 0;
}

impl WritableFile for PosixWritableFile {
    fn new(filename: String, reopen: bool, preallocation_block_size: usize) -> PosixWritableFile {
        let fd;
        let flag = if reopen {
            get_flag() | libc::O_APPEND | libc::O_RDWR
        } else {
            get_flag() | libc::O_TRUNC | libc::O_RDWR
        };
        unsafe {
            fd = libc::open(
                CString::from_vec_unchecked(filename.clone().into_bytes()).as_ptr(),
                flag,
                0o644,
            );
        }
        PosixWritableFile {
            filename_: filename,
            use_direct_io_: true,
            fd_: fd,
            preallocation_block_size_: preallocation_block_size,
            last_preallocated_block_: 0,
            filesize_: 0,
            logical_sector_size_: get_logical_buffer_size(),
        }
    }

    fn append(&mut self, data: Vec<u8>) -> State {
        let State: isize;
        println!("write {:?}", data);
        unsafe {
            State = libc::write(self.fd_, data.as_ptr() as *const libc::c_void, data.len());
        }
        if State < 0 {
            return State::new(Code::KIOError, "cannot append".to_string(), "".to_string());
        }
        self.filesize_ += data.len();
        return State::ok();
    }

    fn sync(&self) -> State {
        let State: i32;
        unsafe {
            State = libc::fsync(self.fd_);
        }
        if State < 0 {
            return State::new(Code::KIOError, "cannot sync".to_string(), "".to_string());
        }
        return State::ok();
    }

    fn close(&self) -> State {
        let State: i32;
        unsafe {
            State = libc::close(self.fd_);
        }
        if State < 0 {
            return State::new(Code::KIOError, "cannot close".to_string(), "".to_string());
        }
        return State::ok();
    }

    #[cfg(target_os = "linux")]
    fn range_sync(&self, offset: i64, nbytes: i64) -> State {
        let State: i32;
        unsafe {
            State = libc::sync_file_range(self.fd_, offset, nbytes, libc::SYNC_FILE_RANGE_WRITE);
        }
        if State < 0 {
            return State::new(
                Code::KIOError,
                "cannot sync_file_range".to_string(),
                "".to_string(),
            );
        }
        return State::ok();
    }

    #[cfg(target_os = "linux")]
    fn allocate(&self, offset: i64, len: i64) -> State {
        let State: i32;
        unsafe {
            State = libc::fallocate(self.fd_, libc::FALLOC_FL_KEEP_SIZE, offset, len);
        }
        if State < 0 {
            return State::new(
                Code::KIOError,
                "cannot allocate".to_string(),
                "".to_string(),
            );
        }
        return State::ok();
    }

    #[cfg(target_os = "linux")]
    fn prepare_write(&mut self, offset: usize, len: usize) {
        if self.preallocation_block_size_ == 0 {
            return;
        }
        let block_size = self.preallocation_block_size_;
        let new_last_preallocated_block = (offset + len + block_size - 1) / block_size;
        if new_last_preallocated_block > self.last_preallocated_block_ {
            let num_spanned_blocks = new_last_preallocated_block - self.last_preallocated_block_;
            self.allocate(
                (block_size * self.last_preallocated_block_) as i64,
                (block_size * num_spanned_blocks) as i64,
            );
            self.last_preallocated_block_ = new_last_preallocated_block;
        }
    }

    fn flush(&self) -> State {
        return State::ok();
    }

    fn use_direct_io(&self) -> bool {
        return self.use_direct_io_;
    }

    fn fcntl(&self) -> bool {
        return unsafe { libc::fcntl(self.fd_, libc::F_GETFL) != -1 };
    }

    fn truncate(&mut self, size: usize) -> State {
        let State: i32;
        unsafe {
            State = libc::ftruncate(self.fd_, size as i64);
        }
        if State < 0 {
            return State::new(
                Code::KIOError,
                "cannot truncate".to_string(),
                "".to_string(),
            );
        } else {
            self.filesize_ = size;
        }
        return State::ok();
    }

    fn get_required_buffer_alignment(&self) -> usize {
        self.logical_sector_size_
    }

    fn positioned_append(&mut self, mut data: Vec<u8>, mut offset: usize) -> State {
        if self.use_direct_io() {
            //println!("offset {} get_logical_buffer_size {}",offset,get_logical_buffer_size());
            //assert!(IsSectorAligned(offset, get_logical_buffer_size()));
            //println!("data len {} get_logical_buffer_size {}",data.len(),get_logical_buffer_size());
            //assert!(IsSectorAligned(data.len(), get_logical_buffer_size()));
            //assert!(IsSectorAligned(data.as_ptr() as usize,get_logical_buffer_size()));
        }
        assert!(offset <= usize::MAX);
        let mut src = data.as_mut_ptr();
        let mut left = data.len();

        let mut done;
        while left != 0 {
            unsafe {
                done = libc::pwrite(self.fd_, src as *const libc::c_void, left, offset as i64);
            }
            if done < 1 {
                unsafe {
                    if *errno_location() as i32 == libc::EINTR {
                        continue;
                    }
                }
                return State::new(
                    Code::KIOError,
                    format!("While pwrite to file at offset {}", offset.to_string()),
                    "".to_string(),
                );
                //IOError("While pwrite to file at offset " + ToString(offset),filename_, errno);
            }
            left -= done as usize;
            offset += done as usize;
            unsafe {
                src = src.offset(done);
            }
        }
        self.filesize_ = offset;
        return State::ok();
    }
}

#[cfg(target_os = "macos")]
fn get_flag_for_posix_sequential_file() -> i32 {
    0
}

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
fn get_flag_for_posix_sequential_file() -> i32 {
    libc::O_DIRECT
}

#[derive(Debug)]
pub struct PosixSequentialFile {
    filename_: String,
    fd_: i32,
    use_direct_io_: bool,
    logical_sector_size_: usize,
    file_: *mut libc::FILE,
}

impl Default for PosixSequentialFile {
    fn default() -> PosixSequentialFile {
        PosixSequentialFile {
            filename_: "".to_string(),
            fd_: 0,
            use_direct_io_: true,
            logical_sector_size_: 0,
            file_: 0 as *mut libc::FILE,
        }
    }
}

impl SequentialFile for PosixSequentialFile {
    fn new(filename: String, options: env::EnvOptions, ptr: &mut PosixSequentialFile) -> State {
        let mut fd = -1;
        let mut flag = libc::O_RDONLY;
        let mut file = 0 as *mut libc::FILE;
        if options.use_direct_reads && !options.use_mmap_reads {
            if cfg!(feature = "CIBO_LITE") {
                return State::new(
                    Code::KIOError,
                    "Direct I/O not supported in cibo lite".to_string(),
                    "".to_string(),
                );
            }
            flag = flag | get_flag_for_posix_sequential_file();
        }
        //flag = get_flag_for_posix_sequential_file();
        loop {
            unsafe {
                fd = libc::open(
                    CString::from_vec_unchecked(filename.clone().into_bytes()).as_ptr(),
                    flag,
                    0o644,
                );
                if !(fd < 0 && *errno_location() as i32 == libc::EINTR) {
                    break;
                }
                println!("{} {} {}", "wait for open", fd, *errno_location());
            }
        }
        if fd < 0 {
            return State::new(
                Code::KIOError,
                "While opening a file for sequentially reading".to_string(),
                "".to_string(),
            );
        }

        SetFD_CLOEXEC(fd, options.clone());
        if options.use_direct_reads && !options.use_mmap_reads {
            #[cfg(target_os = "macos")]
            unsafe {
                if libc::fcntl(fd, libc::F_NOCACHE, 1) == -1 {
                    libc::close(fd);
                    println!("While fcntl NoCache");
                    return State::new(
                        Code::KIOError,
                        "While fcntl NoCache".to_string(),
                        "".to_string(),
                    );
                    //IOError("While fcntl NoCache", fname, errno);
                }
            }
        } else {
            unsafe {
                loop {
                    file = libc::fdopen(fd, b"r".as_ptr() as *const c_char);
                    if !(file == 0 as *mut libc::FILE && *errno_location() as i32 == libc::EINTR) {
                        break;
                    }
                }
                if file == 0 as *mut libc::FILE {
                    libc::close(fd);
                    println!("While opening a file for sequentially read");
                    return State::new(
                        Code::KIOError,
                        "While opening a file for sequentially read".to_string(),
                        "".to_string(),
                    );
                }
            }
        }
        println!("file new {:?}", file);
        *ptr = PosixSequentialFile {
            filename_: filename,
            fd_: fd,
            file_: file,
            use_direct_io_: true,
            logical_sector_size_: get_logical_buffer_size(),
        };
        return State::ok();
    }

    fn skip(&self, n: i64) -> State {
        unsafe {
            if libc::fseek(self.file_, n, libc::SEEK_CUR) > 0 {
                // return IOError("While fseek to skip " + ToString(n) + " bytes", filename_, errno);
                return State::new(
                    Code::KIOError,
                    "While fseek to skip ".to_string() + &n.to_string() + &" bytes".to_string(),
                    "".to_string(),
                );
            }
            return State::ok();
        }
    }

    fn read(&mut self, n: usize, result: &mut Vec<u8>, _scratch: &mut Vec<u8>) -> State {
        let mut s: State = State::ok();
        let mut r = 0;
        let mut scratch: Vec<u8> = vec![0; n];
        unsafe {
            loop {
                r = posix_fread_unlocked(
                    scratch.as_mut_ptr() as *mut libc::c_void,
                    1 as libc::size_t,
                    n as libc::size_t,
                    self.file_,
                );

                if !(libc::ferror(self.file_) > 0
                    && ((*errno_location()) as i32 == libc::EINTR)
                    && r == 0)
                {
                    break;
                }
            }
            println!("fread result len  {:?}", scratch.len());
            //println!("scratch {:?}", scratch);
            result.extend_from_slice(scratch.as_slice());
            println!("fread size {}", r);
            result.split_off(r);

            if r < n {
                println!("feof {}", libc::feof(self.file_));
                if libc::feof(self.file_) == 0 {
                    s = State::new(
                        Code::KIOError,
                        "While reading file sequentially".to_string(),
                        "".to_string(),
                    );
                } else {
                    println!("clear eof err");
                    clearerr(self.file_);
                }
            }
        }
        return s;
    }
}
