
#![feature(box_syntax)]

extern crate misc;
extern crate lsm;

use lsm::IForwardCursor;
use lsm::ILiveValue;
use lsm::ISeekableCursor;
use misc::tempfile;
use std::io::Read;

fn into_utf8(s : String) -> Box<[u8]> {
    s.into_bytes().into_boxed_slice()
}

fn str_to_utf8(s : &str) -> Box<[u8]> {
    // TODO use str::as_bytes()
    s.to_string().into_bytes().into_boxed_slice()
}

fn from_utf8(a: Box<[u8]>) -> String {
    let k = std::string::String::from_utf8(a.into_iter().map(|b| *b).collect()).unwrap();
    k
}

fn key_as_boxed_slice(csr: &lsm::LivingCursor) -> Box<[u8]> {
    csr.key().unwrap().into_boxed_slice()
}

fn key_as_string(csr: &lsm::LivingCursor) -> String {
    from_utf8(key_as_boxed_slice(csr))
}

fn insert_pair_string_string(d: &mut std::collections::BTreeMap<Box<[u8]>, lsm::ValueForStorage>, k: &str, v: &str) {
    d.insert(str_to_utf8(k), lsm::ValueForStorage::Boxed(str_to_utf8(v)));
}

fn insert_pair_string_blob(d: &mut std::collections::BTreeMap<Box<[u8]>, lsm::ValueForStorage>, k: &str, v: lsm::ValueForStorage) {
    d.insert(str_to_utf8(k), v);
}

fn count_keys_forward(csr: &mut lsm::LivingCursor) -> lsm::Result<usize> {
    let mut r = 0;
    try!(csr.first());
    while csr.is_valid() {
        r = r + 1;
        try!(csr.next());
    }
    Ok(r)
}

fn count_keys_backward(csr: &mut lsm::LivingCursor) -> lsm::Result<usize> {
    let mut r = 0;
    try!(csr.last());
    while csr.is_valid() {
        r = r + 1;
        try!(csr.prev());
    }
    Ok(r)
}

fn read_value(b: lsm::LiveValueRef) -> lsm::Result<Box<[u8]>> {
    match b {
        lsm::LiveValueRef::Overflowed(f, page) => {
            let mut a = vec![];
            let mut strm = try!(lsm::OverflowReader::new(f, page));
            try!(strm.read_to_end(&mut a));
            Ok(a.into_boxed_slice())
        },
        lsm::LiveValueRef::Slice(a) => {
            let mut k = Vec::with_capacity(a.len());
            k.extend_from_slice(a);
            Ok(k.into_boxed_slice())
        },
    }
}

#[test]
fn empty_cursor() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("empty_cursor"), lsm::DEFAULT_SETTINGS));
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(!csr.is_valid());
        try!(csr.last());
        assert!(!csr.is_valid());
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn first_prev() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("first_prev"), lsm::DEFAULT_SETTINGS));
        let g = try!(db.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 0, end: 100, step: 1}));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());
        try!(csr.prev());
        assert!(!csr.is_valid());
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn last_next() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("first_prev"), lsm::DEFAULT_SETTINGS));
        let g = try!(db.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 0, end: 100, step: 1}));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.last());
        assert!(csr.is_valid());
        try!(csr.next());
        assert!(!csr.is_valid());
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn seek() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("seek"), lsm::DEFAULT_SETTINGS));
        let g = try!(db.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 0, end: 100, step: 1}));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());
        // TODO constructing the utf8 byte array seems convoluted

        let k = into_utf8(format!("{:08}", 42));
        try!(csr.seek(&lsm::KeyRef::Slice(&k), lsm::SeekOp::Equal));
        assert!(csr.is_valid());

        let k = into_utf8(format!("{:08}", 105));
        try!(csr.seek(&lsm::KeyRef::Slice(&k), lsm::SeekOp::Equal));
        assert!(!csr.is_valid());

        let k = into_utf8(format!("{:08}", 105));
        try!(csr.seek(&lsm::KeyRef::Slice(&k), lsm::SeekOp::GreaterOrEqual));
        assert!(!csr.is_valid());

        let k = into_utf8(format!("{:08}", 105));
        try!(csr.seek(&lsm::KeyRef::Slice(&k), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());
        // TODO get the key

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn lexographic() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("lexicographic"), lsm::DEFAULT_SETTINGS));
        let mut d = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut d, "8", "");
        insert_pair_string_string(&mut d, "10", "");
        insert_pair_string_string(&mut d, "20", "");
        let g = try!(db.write_segment(d));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "10");

        try!(csr.next());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "20");

        try!(csr.next());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "8");

        try!(csr.next());
        assert!(!csr.is_valid());

        // --------
        try!(csr.last());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "8");

        try!(csr.prev());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "20");

        try!(csr.prev());
        assert!(csr.is_valid());
        assert_eq!(key_as_string(&csr), "10");

        try!(csr.prev());
        assert!(!csr.is_valid());

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn seek_cur() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("seek_cur"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        for i in 0 .. 100 {
            let sk = format!("{:03}", i);
            let sv = format!("{}", i);
            insert_pair_string_string(&mut t1, &sk, &sv);
        }
        let mut t2 = std::collections::BTreeMap::new();
        for i in 0 .. 1000 {
            let sk = format!("{:05}", i);
            let sv = format!("{}", i);
            insert_pair_string_string(&mut t2, &sk, &sv);
        }
        let g1 = try!(db.write_segment(t1));
        let g2 = try!(db.write_segment(t2));
        {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g1.unwrap()));
            try!(lck.commit_segment(g2.unwrap()));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("00001")), lsm::SeekOp::Equal));
        assert!(csr.is_valid());
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn weird() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("weird"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        for i in 0 .. 100 {
            let sk = format!("{:03}", i);
            let sv = format!("{}", i);
            insert_pair_string_string(&mut t1, &sk, &sv);
        }
        let mut t2 = std::collections::BTreeMap::new();
        for i in 0 .. 1000 {
            let sk = format!("{:05}", i);
            let sv = format!("{}", i);
            insert_pair_string_string(&mut t2, &sk, &sv);
        }
        let g1 = try!(db.write_segment(t1));
        let g2 = try!(db.write_segment(t2));
        {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g1.unwrap()));
            try!(lck.commit_segment(g2.unwrap()));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        for _ in 0 .. 100 {
            try!(csr.next());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 50 {
            try!(csr.prev());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 100 {
            try!(csr.next());
            assert!(csr.is_valid());
            try!(csr.next());
            assert!(csr.is_valid());
            try!(csr.prev());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 50 {
            let kboxed = key_as_boxed_slice(&csr);
            let k = lsm::KeyRef::Slice(&kboxed);
            try!(csr.seek(&k, lsm::SeekOp::Equal));
            assert!(csr.is_valid());
            try!(csr.next());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 50 {
            let kboxed = key_as_boxed_slice(&csr);
            let k = lsm::KeyRef::Slice(&kboxed);
            try!(csr.seek(&k, lsm::SeekOp::Equal));
            assert!(csr.is_valid());
            try!(csr.prev());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 50 {
            let kboxed = key_as_boxed_slice(&csr);
            let k = lsm::KeyRef::Slice(&kboxed);
            try!(csr.seek(&k, lsm::SeekOp::LessOrEqual));
            assert!(csr.is_valid());
            try!(csr.prev());
            assert!(csr.is_valid());
        }
        for _ in 0 .. 50 {
            let kboxed = key_as_boxed_slice(&csr);
            let k = lsm::KeyRef::Slice(&kboxed);
            try!(csr.seek(&k, lsm::SeekOp::GreaterOrEqual));
            assert!(csr.is_valid());
            try!(csr.next());
            assert!(csr.is_valid());
        }
        // got the following value from the debugger.
        // just want to make sure that it doesn't change
        // and all combos give the same answer.
        assert_eq!(key_as_string(&csr), "00148");
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn no_le_ge_multicursor() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("no_le_ge_multicursor"), lsm::DEFAULT_SETTINGS));

        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "g", "7");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t2, "e", "5");
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("a")), lsm::SeekOp::LessOrEqual));
        assert!(!csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("d")), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("f")), lsm::SeekOp::GreaterOrEqual));
        assert!(csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("h")), lsm::SeekOp::GreaterOrEqual));
        assert!(!csr.is_valid());

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn empty_val() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("empty_val"), lsm::DEFAULT_SETTINGS));

        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "_", "");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("_")), lsm::SeekOp::Equal));
        assert!(csr.is_valid());
        let q = try!(csr.value());
        let (len, _) = try!(q.read());
        assert_eq!(0, len);

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn delete_not_there() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("delete_not_there"), lsm::DEFAULT_SETTINGS));

        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "a", "1");
        insert_pair_string_string(&mut t1, "b", "2");
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "d", "4");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, "e", lsm::ValueForStorage::Tombstone);
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());
        assert_eq!(4, try!(count_keys_forward(&mut csr)));
        assert_eq!(4, try!(count_keys_backward(&mut csr)));

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn delete_nothing_there() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("delete_nothing_there"), lsm::DEFAULT_SETTINGS));

        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, "e", lsm::ValueForStorage::Tombstone);
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());
        assert_eq!(0, try!(count_keys_forward(&mut csr)));
        assert_eq!(0, try!(count_keys_backward(&mut csr)));

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn simple_tombstone() {
    fn f(del: &str) -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("simple_tombstone"), lsm::DEFAULT_SETTINGS));

        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "a", "1");
        insert_pair_string_string(&mut t1, "b", "2");
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "d", "4");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, del, lsm::ValueForStorage::Tombstone);
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());
        assert_eq!(3, try!(count_keys_forward(&mut csr)));
        assert_eq!(3, try!(count_keys_backward(&mut csr)));

        Ok(())
    }
    assert!(f("a").is_ok());
    assert!(f("b").is_ok());
    assert!(f("c").is_ok());
    assert!(f("d").is_ok());
}

#[test]
fn many_segments() {
    fn f() -> lsm::Result<bool> {
        let db = try!(lsm::DatabaseFile::new(tempfile("many_segments"), lsm::DEFAULT_SETTINGS));

        const NUM : usize = 5000;
        const EACH : usize = 10;

        for i in 0 .. NUM {
            let g = try!(db.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: i * EACH, end: (i+1) * EACH, step: 1})).unwrap();
            {
                let lck = try!(db.get_write_lock());
                try!(lck.commit_segment(g));
            }
        }

        let res : lsm::Result<bool> = Ok(true);
        res
    }
    assert!(f().is_ok());
}

#[test]
fn one_blob() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("one_blob"), lsm::DEFAULT_SETTINGS));

        const LEN: usize = 100000;

        let mut v = Vec::new();
        for i in 0 .. LEN {
            v.push(i as u8);
        }
        assert_eq!(LEN, v.len());
        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, "e", lsm::ValueForStorage::Boxed(v.into_boxed_slice()));
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());
        assert_eq!(1, try!(count_keys_forward(&mut csr)));
        assert_eq!(1, try!(count_keys_backward(&mut csr)));

        try!(csr.first());
        assert!(csr.is_valid());
        let v = try!(csr.value());
        let (len, mut strm) = try!(v.read());
        assert_eq!(LEN as u64, len);
        let mut a = vec![];
        try!(strm.read_to_end(&mut a));
        // TODO compare the actual bytes

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn one_blob_unknown_len() {
    fn f(given_len: usize) -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("one_blob_unknown_len"), lsm::DEFAULT_SETTINGS));

        let mut v = Vec::new();
        for i in 0 .. given_len {
            v.push(i as u8);
        }
        let v = v.into_boxed_slice();
        assert_eq!(given_len, v.len());
        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, "e", lsm::ValueForStorage::UnknownLen(box misc::ByteBufRead::new(v)));
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }

        let mut csr = try!(db.open_cursor());
        assert_eq!(1, try!(count_keys_forward(&mut csr)));
        assert_eq!(1, try!(count_keys_backward(&mut csr)));

        try!(csr.first());
        assert!(csr.is_valid());
        let q = try!(csr.value());
        let (len, mut strm) = try!(q.read());
        assert_eq!(given_len as u64, len);
        let mut a = vec![];
        try!(strm.read_to_end(&mut a));
        // TODO compare the actual bytes

        Ok(())
    }
    assert!(f(100_000).is_ok());
    assert!(f(40_000).is_ok());
    assert!(f(20_000).is_ok());
}

#[test]
fn no_le_ge() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("no_le_ge"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "g", "7");
        insert_pair_string_string(&mut t1, "e", "5");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("a")), lsm::SeekOp::LessOrEqual));
        assert!(!csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("d")), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("f")), lsm::SeekOp::GreaterOrEqual));
        assert!(csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("h")), lsm::SeekOp::GreaterOrEqual));
        assert!(!csr.is_valid());

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn seek_ge_le_bigger() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("seek_ge_le_bigger"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        for i in 0 .. 10000 {
            let sk = format!("{}", i*2);
            let sv = format!("{}", i);
            insert_pair_string_string(&mut t1, &sk, &sv);
        }
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("8088")), lsm::SeekOp::Equal));
        assert!(csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("8087")), lsm::SeekOp::Equal));
        assert!(!csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("8087")), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());
        assert_eq!("8086", key_as_string(&csr));

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("8087")), lsm::SeekOp::GreaterOrEqual));
        assert!(csr.is_valid());
        assert_eq!("8088", key_as_string(&csr));

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn seek_ge_le() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("seek_ge_le"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "a", "1");
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "e", "5");
        insert_pair_string_string(&mut t1, "g", "7");
        insert_pair_string_string(&mut t1, "i", "9");
        insert_pair_string_string(&mut t1, "k", "11");
        insert_pair_string_string(&mut t1, "m", "13");
        insert_pair_string_string(&mut t1, "o", "15");
        insert_pair_string_string(&mut t1, "q", "17");
        insert_pair_string_string(&mut t1, "s", "19");
        insert_pair_string_string(&mut t1, "u", "21");
        insert_pair_string_string(&mut t1, "w", "23");
        insert_pair_string_string(&mut t1, "y", "25");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        assert_eq!(13, try!(count_keys_forward(&mut csr)));
        assert_eq!(13, try!(count_keys_backward(&mut csr)));

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("n")), lsm::SeekOp::Equal));
        assert!(!csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("n")), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());
        assert_eq!("m", key_as_string(&csr));

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("n")), lsm::SeekOp::GreaterOrEqual));
        assert!(csr.is_valid());
        assert_eq!("o", key_as_string(&csr));

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn tombstone() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("tombstone"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "a", "1");
        insert_pair_string_string(&mut t1, "b", "2");
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "d", "4");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_blob(&mut t2, "b", lsm::ValueForStorage::Tombstone);
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        // TODO it would be nice to check the multicursor without the living wrapper
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());
        assert_eq!("a", key_as_string(&csr));
        assert_eq!("1", from_utf8(read_value(csr.value().unwrap()).unwrap()));

        try!(csr.next());
        assert!(csr.is_valid());
        assert_eq!("c", key_as_string(&csr));
        assert_eq!("3", from_utf8(read_value(csr.value().unwrap()).unwrap()));

        try!(csr.next());
        assert!(csr.is_valid());
        assert_eq!("d", key_as_string(&csr));
        assert_eq!("4", from_utf8(read_value(csr.value().unwrap()).unwrap()));

        try!(csr.next());
        assert!(!csr.is_valid());

        assert_eq!(3, try!(count_keys_forward(&mut csr)));
        assert_eq!(3, try!(count_keys_backward(&mut csr)));

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("b")), lsm::SeekOp::Equal));
        assert!(!csr.is_valid());

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("b")), lsm::SeekOp::LessOrEqual));
        assert!(csr.is_valid());
        assert_eq!("a", key_as_string(&csr));
        try!(csr.next());
        assert!(csr.is_valid());
        assert_eq!("c", key_as_string(&csr));

        try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("b")), lsm::SeekOp::GreaterOrEqual));
        assert!(csr.is_valid());
        assert_eq!("c", key_as_string(&csr));
        try!(csr.prev());
        assert_eq!("a", key_as_string(&csr));

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn overwrite() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("overwrite"), lsm::DEFAULT_SETTINGS));
        let mut t1 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t1, "a", "1");
        insert_pair_string_string(&mut t1, "b", "2");
        insert_pair_string_string(&mut t1, "c", "3");
        insert_pair_string_string(&mut t1, "d", "4");
        let g = try!(db.write_segment(t1));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        fn getb(db: &lsm::DatabaseFile) -> lsm::Result<String> {
            let mut csr = try!(db.open_cursor());
            try!(csr.seek(&lsm::KeyRef::Slice(&str_to_utf8("b")), lsm::SeekOp::Equal));
            Ok(from_utf8(read_value(csr.value().unwrap()).unwrap()))
        }
        assert_eq!("2", getb(&db).unwrap());
        let mut t2 = std::collections::BTreeMap::new();
        insert_pair_string_string(&mut t2, "b", "5");
        let g = try!(db.write_segment(t2));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        assert_eq!("5", getb(&db).unwrap());

        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn blobs_of_many_sizes() {
    fn f() -> lsm::Result<()> {
        let settings = lsm::DbSettings {
                default_page_size : 256,
                pages_per_block : 4,
                .. lsm::DEFAULT_SETTINGS
            };
        let db = try!(lsm::DatabaseFile::new(tempfile("blobs_of_many_sizes"), settings));
        // TODO why doesn't Box<[u8]> support clone?
        // for now, we have a function to generate the pile we need, and we call it twice
        fn gen() -> std::collections::BTreeMap<Box<[u8]>, lsm::ValueForStorage> {
            let mut t1 = std::collections::BTreeMap::new();
            for i in 200 .. 1500 {
                let k = format!("{}", i);
                let mut v = String::new();
                for j in 0 .. i {
                    let s = format!("{}", j);
                    v.push_str(&s);
                }
                insert_pair_string_string(&mut t1, &k, &v);
            }
            t1
        }
        println!("writing segment");
        let g = try!(db.write_segment(gen()));
        println!("wrote segment: {:?}", g);
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            println!("got write lock");
            try!(lck.commit_segment(g));
            println!("committed segment");
        }
        println!("opening cursor");
        let mut csr = try!(db.open_cursor());
        println!("got cursor");
        let t1 = gen(); // generate another copy
        for (k, v) in t1 {
            if let lsm::ValueForStorage::Boxed(v) = v {
                println!("k: {:?}", k);
                try!(csr.seek(&lsm::KeyRef::Slice(&k), lsm::SeekOp::Equal));
                assert!(csr.is_valid());
                println!("    valid");
                let q = try!(csr.value());
                let (len, mut strm) = try!(q.read());
                assert_eq!(v.len() as u64, len);
                let mut a = vec![];
                try!(strm.read_to_end(&mut a));
                let a = a.into_boxed_slice();
                assert_eq!(v, a);
            } else {
                unreachable!();
            }
        }
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn write_then_read() {
    fn f() -> lsm::Result<()> {
        fn write(name: &str) -> lsm::Result<()> {
            let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::DEFAULT_SETTINGS));
            let mut d = std::collections::BTreeMap::new();
            for i in 1 .. 100 {
                let s = format!("{}", i);
                insert_pair_string_string(&mut d, &s, &s);
            }
            let g = try!(db.write_segment(d));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
            let mut d = std::collections::BTreeMap::new();
            insert_pair_string_blob(&mut d, "73", lsm::ValueForStorage::Tombstone);
            let g = try!(db.write_segment(d));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
            Ok(())
        }

        fn read(name: &str) -> lsm::Result<()> {
            let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::DEFAULT_SETTINGS));
            let mut csr = try!(db.open_cursor());
            try!(csr.seek(&lsm::KeyRef::Slice(&into_utf8(format!("{}", 42))), lsm::SeekOp::Equal));
            assert!(csr.is_valid());
            try!(csr.next());
            assert_eq!("43", key_as_string(&csr));
            try!(csr.seek(&lsm::KeyRef::Slice(&into_utf8(format!("{}", 73))), lsm::SeekOp::Equal));
            assert!(!csr.is_valid());
            try!(csr.seek(&lsm::KeyRef::Slice(&into_utf8(format!("{}", 73))), lsm::SeekOp::LessOrEqual));
            assert!(csr.is_valid());
            assert_eq!("72", key_as_string(&csr));
            try!(csr.next());
            assert!(csr.is_valid());
            assert_eq!("74", key_as_string(&csr));
            Ok(())
        }

        let name = tempfile("write_then_read");
        try!(write(&name));
        try!(read(&name));
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn prefix_compression() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("prefix_compression"), lsm::DEFAULT_SETTINGS));
        let mut d = std::collections::BTreeMap::new();
        for i in 1 .. 10000 {
            let s = format!("{}", i);
            insert_pair_string_string(&mut d, &("prefix_compression".to_string() + &s), &s);
        }
        let g = try!(db.write_segment(d));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn threads() {
    fn f() -> lsm::Result<()> {
        use std::sync::Arc;
        use std::thread;

        let settings = lsm::DbSettings {
                default_page_size : 256,
                pages_per_block : 4,
                .. lsm::DEFAULT_SETTINGS
            };
        let db = try!(lsm::DatabaseFile::new(tempfile("threads"), settings));
        let data = Arc::new(db);

        let h1 = {
            let data = data.clone();
            let h = thread::spawn(move || -> lsm::Result<()> {
                let _g = try!(data.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 0, end: 10000, step: 1}));
                Ok(())
            });
            h
        };

        let h2 = {
            let data = data.clone();
            let h = thread::spawn(move || -> lsm::Result<()> {
                let _g = try!(data.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 20000, end: 30000, step: 1}));
                Ok(())
            });
            h
        };

        let r1 = h1.join();
        let r2 = h2.join();
        assert!(r1.is_ok());
        assert!(r2.is_ok());

        Ok(())
    }

    assert!(f().is_ok());
}

#[test]
fn key_ref() {
    fn f() -> lsm::Result<()> {
        let db = try!(lsm::DatabaseFile::new(tempfile("key_ref"), lsm::DEFAULT_SETTINGS));
        let g = try!(db.write_segment_from_sorted_sequence(lsm::GenerateNumbers {cur: 0, end: 100, step: 1}));
        if let Some(g) = g {
            let lck = try!(db.get_write_lock());
            try!(lck.commit_segment(g));
        }
        let mut csr = try!(db.open_cursor());
        try!(csr.first());
        assert!(csr.is_valid());

        while csr.is_valid() {
            {
                // KeyRef takes an immutable reference on the cursor.
                // which means you can't call next() on that cursor
                // until the reference goes out of scope.  So this
                // is in its own block.
                let q = csr.key();
                println!("{:?}", q);
            }
            try!(csr.next());
        }
        Ok(())
    }
    assert!(f().is_ok());
}

#[test]
fn threads_with_weird_pairs() {
    fn f(klen: usize, vlen: usize, threads: usize, pairs: usize) -> lsm::Result<()> {
        use std::sync::Arc;
        use std::thread;

        let settings = lsm::DbSettings {
                default_page_size : 256,
                pages_per_block : 2,
                .. lsm::DEFAULT_SETTINGS
            };
        let db = try!(lsm::DatabaseFile::new(tempfile("threads_with_overflows"), settings));
        let data = Arc::new(db);

        let mut handles = Vec::new();
        
        for i in 0 .. threads {
            let h = {
                let data = data.clone();
                let h = thread::spawn(move || -> lsm::Result<()> {
                    let _g = try!(data.write_segment_from_sorted_sequence(lsm::GenerateWeirdPairs {cur: i*10, end: i*10+pairs, klen: klen, vlen: vlen}));
                    Ok(())
                });
                h
            };
            handles.push(h);
        }

        for h in handles {
            let r = h.join();
            assert!(r.is_ok());
        }

        Ok(())
    }

    let r = f(100, 100, 10, 1000); 
    assert!(r.is_ok());

    let r = f(100, 1000, 10, 100); 
    assert!(r.is_ok());

    let r = f(1000, 100, 10, 100); 
    assert!(r.is_ok());

    let r = f(1000, 1000, 10, 10); 
    assert!(r.is_ok());

}

