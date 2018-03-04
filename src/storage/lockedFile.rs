use std::fs::File;
use std::fs::{self, DirBuilder};

struct lockedfile {
    file: File,
}

fn lock_file_name(dbname: String) -> String {
    dbname + "/LOC"
}

fn LockFile(fname: String) {
    DirBuilder::new().recursive(true).create(fname).unwrap();
}

//#[repr(C)]
//pub struct MY_FILE {
//    pub _p: libc::c_char,
//    pub _r: libc::c_int,
//    pub _w: libc::c_int,
//    pub _flags: libc::c_short,
//    pub _file: libc::c_short,
//    // ...
//}

//// ...

//unsafe {
//    let fp = libc::fdopen(libc::STDOUT_FILENO, &('w' as libc::c_char));
//    let fp = &mut *(fp as *mut MY_FILE);
//    let is_unbuffered = (fp._flags & libc::_IONBF as i16) != 0;
//}
