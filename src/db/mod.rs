mod log_format;
pub mod log_reader;
pub mod log_writer;

use db::log_reader::Reader;
use db::log_writer::Writer;
use env;
use env::io_posix::PosixSequentialFile;
use env::io_posix::PosixWritableFile;
use env::EnvOptions;
use env::SequentialFile;
use env::WritableFile;
use util::file_reader_writer::SequentialFileReader;
use util::file_reader_writer::WritableFileWriter;
use util::hash::crc32;

#[test]
fn test_wal() {
    {
        let mut fd = PosixWritableFile::new("test".to_string(), false, 1024);
        let mut op: EnvOptions = EnvOptions::default();
        op.writable_file_max_buffer_size = 50;
        let mut writer = WritableFileWriter::new(fd, op);
        let mut wal = Writer::new(writer, 0, false, true);

        let input = vec![1, 2, 3];
        wal.add_record(input);
        let input = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        wal.add_record(input);
        let input = vec![1, 2, 3];
        wal.add_record(input);
        let input = vec![1, 2];
        wal.add_record(input);
    }
    {
        let mut pf: PosixSequentialFile = PosixSequentialFile::default();
        let mut op: EnvOptions = EnvOptions::default();
        let mut state = PosixSequentialFile::new("test".to_string(), op, &mut pf);
        let mut sf = SequentialFileReader::new(pf);
        let mut reader = Reader::new(sf, 0, 0, true);
        let mut record: Vec<u8> = Vec::new();
        let mut scratch: Vec<u8> = Vec::new();

        {
            reader.readRecord(
                &mut record,
                &mut scratch,
                env::WALRecoveryMode::kAbsoluteConsistency,
            );
        }
        assert_eq!(record, vec![1, 2, 3]);
        record.clear();
        scratch.clear();
        {
            reader.readRecord(
                &mut record,
                &mut scratch,
                env::WALRecoveryMode::kAbsoluteConsistency,
            );
        }
        assert_eq!(record, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }
}
