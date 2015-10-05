﻿/*
    Copyright 2014-2015 Zumero, LLC

    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        http://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

#![feature(box_syntax)]
#![feature(associated_consts)]
#![feature(vec_push_all)]
#![feature(iter_arith)]

use std::collections::HashMap;
use std::collections::HashSet;

extern crate bson;

extern crate misc;
extern crate elmo;
extern crate lsm;

use lsm::ICursor;

pub type Result<T> = elmo::Result<T>;

/*

this doesn't help.

pub struct WrapError {
    err: lsm::Error,
}

impl From<WrapError> for elmo::Error {
    fn from(err: WrapError) -> elmo::Error {
        elmo::Error::Whatever(box err.err)
    }
}

impl From<lsm::Error> for WrapError {
    fn from(err: lsm::Error) -> WrapError {
        WrapError {
            err: err
        }
    }
}

impl Into<WrapError> for lsm::Error {
    fn into(self) -> WrapError {
        WrapError {
            err: self
        }
    }
}
*/

/*

the compiler won't allow this

the impl does not reference any types defined in this crate; 
only traits defined in the current crate can be implemented for arbitrary types

impl From<lsm::Error> for elmo::Error {
    fn from(err: lsm::Error) -> elmo::Error {
        elmo::Error::Whatever(box err)
    }
}
*/

struct MyIndexPrep {
    index_id: u64,
    spec: bson::Document,
    options: bson::Document,
    normspec: Vec<(String,elmo::IndexType)>,
    weights: Option<HashMap<String,i32>>,
    // TODO maybe keep the options we need here directly, sparse and unique
}

struct MyCollectionWriter {
    // TODO might want db and coll names here for caching
    indexes: Vec<MyIndexPrep>,
    collection_id: u64,
}

struct MyCollectionReader {
    seq: Box<Iterator<Item=Result<elmo::Row>>>,

    // TODO need counts here
}

// TODO LivingCursorBsonValueIterator
// and PrefixCursorBsonValueIterator 
// are basically identical.

struct LivingCursorBsonValueIterator {
    cursor: lsm::LivingCursor,
}

impl LivingCursorBsonValueIterator {
    fn iter_next(&mut self) -> Result<Option<elmo::Row>> {
        try!(self.cursor.Next().map_err(elmo::wrap_err));
        if self.cursor.IsValid() {
            let v = try!(self.cursor.LiveValueRef().map_err(elmo::wrap_err));
            let v = try!(v.map(lsm_map_to_bson).map_err(elmo::wrap_err));
            let v = v.into_value();
            let row = elmo::Row {
                doc: v,
                pos: None,
                score: None,
            };
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }
}

impl Iterator for LivingCursorBsonValueIterator {
    type Item = Result<elmo::Row>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.iter_next() {
            Err(e) => {
                return Some(Err(e));
            },
            Ok(v) => {
                match v {
                    None => {
                        return None;
                    },
                    Some(v) => {
                        return Some(Ok(v));
                    }
                }
            },
        }
    }
}

struct PrefixCursorBsonValueIterator {
    cursor: lsm::PrefixCursor,
}

impl PrefixCursorBsonValueIterator {
    fn iter_next(&mut self) -> Result<Option<elmo::Row>> {
        try!(self.cursor.Next().map_err(elmo::wrap_err));
        if self.cursor.IsValid() {
            let v = try!(self.cursor.LiveValueRef().map_err(elmo::wrap_err));
            let v = try!(v.map(lsm_map_to_bson).map_err(elmo::wrap_err));
            let v = v.into_value();
            let row = elmo::Row {
                doc: v,
                pos: None,
                score: None,
            };
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }
}

impl Iterator for PrefixCursorBsonValueIterator {
    type Item = Result<elmo::Row>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.iter_next() {
            Err(e) => {
                return Some(Err(e));
            },
            Ok(v) => {
                match v {
                    None => {
                        return None;
                    },
                    Some(v) => {
                        return Some(Ok(v));
                    }
                }
            },
        }
    }
}

struct RangeCursorVarintIterator {
    cursor: lsm::RangeCursor,
}

impl RangeCursorVarintIterator {
    fn iter_next(&mut self) -> Result<Option<u64>> {
        try!(self.cursor.Next().map_err(elmo::wrap_err));
        if self.cursor.IsValid() {
            let v = try!(self.cursor.LiveValueRef().map_err(elmo::wrap_err));
            let v = try!(v.map(lsm_map_to_varint).map_err(elmo::wrap_err));
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }
}

impl Iterator for RangeCursorVarintIterator {
    type Item = Result<u64>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.iter_next() {
            Err(e) => {
                return Some(Err(e));
            },
            Ok(v) => {
                match v {
                    None => {
                        return None;
                    },
                    Some(v) => {
                        return Some(Ok(v));
                    }
                }
            },
        }
    }
}

struct MyReader {
    myconn: std::rc::Rc<MyConn>,
}

struct MyWriter<'a> {
    myconn: std::rc::Rc<MyConn>,
    tx: std::sync::MutexGuard<'a, lsm::WriteLock>,
    pending: HashMap<Box<[u8]>,lsm::Blob>,
    // TODO cache the collection writer
    max_collection_id: Option<u64>,
    max_record_id: HashMap<u64, u64>,
}

struct MyConn {
    conn: lsm::db,
}

struct MyPublicConn {
    myconn: std::rc::Rc<MyConn>,
}

// TODO should all these value encodings be switching around to have
// the collid first, before the tag?  drop_collection gets trivial.
// and the collection gets good locality of storage.

// TODO should we have record ids?  or just have the _id of each record
// be its actual key?  
//
// the pk can be big, and it will be duplicated,
// once in the key, and once in the bson doc itself.
//
// the pk or id is also duplicated in the index entries.
// and in their backlinks.
//
// if we don't have a recid, how would we store a document that doesn't
// have any _id at all?

/// key:
///     (tag)
///     db name (len + str)
///     coll name (len + str)
/// value:
///     collid (varint)
pub const NAME_TO_COLLECTION_ID: u8 = 10;

/// key:
///     (tag)
///     collid (varint)
/// value:
///     properties (bson):
///         d: db name (str)
///         c: coll name (str)
///         o: options (document)
pub const COLLECTION_ID_TO_PROPERTIES: u8 = 11;
pub const COLLECTION_ID_BOUNDARY: u8 = COLLECTION_ID_TO_PROPERTIES + 1;

/// key:
///     (tag)
///     collid (varint)
///     index name (len + str)
/// value:
///     indexid (varint)
pub const NAME_TO_INDEX_ID: u8 = 20;

/// key:
///     (tag)
///     collid (varint)
///     indexid (varint)
/// value:
///     properties (bson):
///         n: name (str)
///         s: spec (bson)
///         o: options (bson)
pub const INDEX_ID_TO_PROPERTIES: u8 = 21;

pub const PRIMARY_INDEX_ID: u64 = 0;

/// key:
///     (tag)
///     collid (varint)
///     recid (varint)
/// value:
///     doc (bson)
pub const RECORD: u8 = 30;

/// key:
///     (tag)
///     collid (varint)
///     indexid (varint)
///     k (len + bytes)
///     recid (varint) (not present when index option unique)
/// value:
///     recid (varint) (present only when index option unique?)
pub const INDEX_ENTRY: u8 = 40;

/// key:
///     (tag)
///     collid (varint)
///     indexid (varint)
///     recid (varint)
///     (complete index key)
/// value:
///    (none)
pub const RECORD_ID_TO_INDEX_ENTRY: u8 = 41;

fn encode_key_name_to_collection_id(db: &str, coll: &str) -> Box<[u8]> {
    // TODO capacity
    let mut k = vec![];
    k.push(NAME_TO_COLLECTION_ID);

    // From the mongo docs:
    // The maximum length of the collection namespace, which includes the database name, the dot
    // (.) separator, and the collection name (i.e. <database>.<collection>), is 120 bytes.

    let b = db.as_bytes();
    k.push(b.len() as u8);
    k.push_all(b);

    let b = coll.as_bytes();
    k.push(b.len() as u8);
    k.push_all(b);

    k.into_boxed_slice()
}

fn decode_string_from_key(k: &lsm::KeyRef, cur: usize) -> Result<(String, usize)> {
    // TODO should we treat the len before the string as a varint instead of always a byte?
    let len = try!(k.u8_at(cur).map_err(elmo::wrap_err)) as usize;
    let cur = cur + 1;
    let s = try!(k.map_range(cur, cur + len, lsm_map_to_string).map_err(elmo::wrap_err));
    let cur = cur + len;
    Ok((s, cur))
}

fn decode_varint_from_key(k: &lsm::KeyRef, cur: usize) -> Result<(u64, usize)> {
    let first_byte = try!(k.u8_at(cur).map_err(elmo::wrap_err));
    let len = misc::varint::first_byte_to_len(first_byte);
    let v = try!(k.map_range(cur, cur + len, lsm_map_to_varint).map_err(elmo::wrap_err));
    let cur = cur + len;
    Ok((v, cur))
}

fn decode_key_name_to_collection_id(k: &lsm::KeyRef) -> Result<(String, String)> {
    // k[0] must be NAME_TO_COLLECTION_ID
    let cur = 1;
    let (db, cur) = try!(decode_string_from_key(k, cur));
    let (coll, cur) = try!(decode_string_from_key(k, cur));
    Ok((db, coll))
}

fn decode_key_name_to_index_id(k: &lsm::KeyRef) -> Result<(u64, String)> {
    // k[0] must be NAME_TO_INDEX_ID
    let cur = 1;
    let (collection_id, cur) = try!(decode_varint_from_key(k, cur));
    let (name, cur) = try!(decode_string_from_key(k, cur));
    Ok((collection_id, name))
}

fn decode_key_record(k: &lsm::KeyRef) -> Result<(u64, u64)> {
    // k[0] must be RECORD
    let cur = 1;
    let (collection_id, cur) = try!(decode_varint_from_key(k, cur));
    let (record_id, cur) = try!(decode_varint_from_key(k, cur));
    Ok((collection_id, record_id))
}

fn decode_key_collection_id_to_properties(k: &lsm::KeyRef) -> Result<(u64)> {
    // k[0] must be COLLECTION_ID_TO_PROPERTIES
    let cur = 1;
    let (collection_id, cur) = try!(decode_varint_from_key(k, cur));
    Ok(collection_id)
}

fn push_varint(v: &mut Vec<u8>, n: u64) {
    let mut buf = [0; 9];
    let mut cur = 0;
    misc::varint::write(&mut buf, &mut cur, n);
    v.push_all(&buf[0 .. cur]);
}

fn encode_key_tag_and_varint(tag: u8, id: u64) -> Vec<u8> {
    // TODO capacity
    let mut k = vec![];
    k.push(tag);

    push_varint(&mut k, id);

    k
}

fn encode_key_collection_id_to_properties(collection_id: u64) -> Vec<u8> {
    encode_key_tag_and_varint(COLLECTION_ID_TO_PROPERTIES, collection_id)
}

fn encode_key_index_id_to_properties(collection_id: u64, index_id: u64) -> Vec<u8> {
    // TODO capacity
    let mut k = vec![];
    k.push(INDEX_ID_TO_PROPERTIES);
    push_varint(&mut k, collection_id);
    push_varint(&mut k, index_id);
    k
}

fn encode_key_name_to_index_id(collection_id: u64, name: &str) -> Vec<u8> {
    // TODO capacity
    let mut k = vec![];
    k.push(NAME_TO_INDEX_ID);

    // From the mongo docs:
    // The maximum length of the collection namespace, which includes the database name, the dot
    // (.) separator, and the collection name (i.e. <database>.<collection>), is 120 bytes.

    let ba = u64_to_boxed_varint(collection_id);
    k.push_all(&ba);

    let b = name.as_bytes();
    k.push(b.len() as u8);
    k.push_all(b);

    k
}

fn lsm_map_to_string(ba: &[u8]) -> lsm::Result<String> {
    let s = try!(std::str::from_utf8(&ba));
    Ok(String::from(s))
}

fn lsm_map_to_varint(ba: &[u8]) -> lsm::Result<u64> {
    let mut cur = 0;
    let n = misc::varint::read(ba, &mut cur);
    // TODO assert cur used up all of ba?
    Ok(n)
}

fn u64_to_boxed_varint(n: u64) -> Box<[u8]> {
    let mut buf = [0; 9];
    let mut cur = 0;
    misc::varint::write(&mut buf, &mut cur, n);
    let mut v = Vec::with_capacity(cur);
    v.push_all(&buf[0 .. cur]);
    let v = v.into_boxed_slice();
    v
}

fn lsm_map_to_bson(ba: &[u8]) -> lsm::Result<bson::Document> {
    let r = bson::Document::from_bson(ba);
    let r = r.map_err(lsm::wrap_err);
    r
}

fn find_record(cursor: &mut lsm::LivingCursor, collection_id: u64, id: &bson::Value) -> Result<u64> {
    let mut k = vec![];
    k.push(INDEX_ENTRY);
    push_varint(&mut k, collection_id);
    push_varint(&mut k, PRIMARY_INDEX_ID);
    let ba = bson::Value::encode_one_for_index(id, false);
    k.push_all(&ba);
    match try!(get_value_for_key_as_varint(cursor, &k)) {
        Some(record_id) => Ok(record_id),
        None => return Err(elmo::Error::Misc(String::from("record not found"))),
    }
}
fn get_value_for_key_as_varint(cursor: &mut lsm::LivingCursor, k: &[u8]) -> Result<Option<u64>> {
    try!(cursor.SeekRef(&lsm::KeyRef::for_slice(&k), lsm::SeekOp::SEEK_EQ).map_err(elmo::wrap_err));
    if cursor.IsValid() {
        let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
        let id = try!(v.map(lsm_map_to_varint).map_err(elmo::wrap_err));
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

fn get_value_for_key_as_bson(cursor: &mut lsm::LivingCursor, k: &[u8]) -> Result<Option<bson::Document>> {
    try!(cursor.SeekRef(&lsm::KeyRef::for_slice(&k), lsm::SeekOp::SEEK_EQ).map_err(elmo::wrap_err));
    if cursor.IsValid() {
        let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
        let id = try!(v.map(lsm_map_to_bson).map_err(elmo::wrap_err));
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

impl MyConn {
    fn get_reader_collection_scan(&self, db: &str, coll: &str) -> Result<MyCollectionReader> {
        // check to see if the collection exists and get its id
        let k = encode_key_name_to_collection_id(db, coll);
        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
            None => {
                let rdr = 
                    MyCollectionReader {
                        seq: box std::iter::empty(),
                    };
                Ok(rdr)
            },
            Some(collection_id) => {
                let mut k = vec![];
                k.push(RECORD);
                push_varint(&mut k, collection_id);
                let mut cursor = lsm::PrefixCursor::new(cursor, k.into_boxed_slice());
                try!(cursor.First().map_err(elmo::wrap_err));
                let seq = 
                    PrefixCursorBsonValueIterator {
                        cursor: cursor,
                    };
                let rdr = 
                    MyCollectionReader {
                        seq: box seq,
                    };
                Ok(rdr)
            },
        }
    }

    fn get_reader_text_index_scan(&self, myconn: std::rc::Rc<MyConn>, commit_on_drop: bool, ndx: &elmo::IndexInfo, eq: elmo::QueryKey, terms: Vec<elmo::TextQueryTerm>) -> Result<MyCollectionReader> {
        unimplemented!();
    }

    fn get_reader_regular_index_scan(&self, ndx: &elmo::IndexInfo, bounds: elmo::QueryBounds) -> Result<MyCollectionReader> {
        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let collection_id = 
            match try!(get_value_for_key_as_varint(&mut cursor, &encode_key_name_to_collection_id(&ndx.db, &ndx.coll))) {
                Some(id) => id,
                None => return Err(elmo::Error::Misc(String::from("collection does not exist"))),
            };
        let index_id = 
            match try!(get_value_for_key_as_varint(&mut cursor, &encode_key_name_to_index_id(collection_id, &ndx.name))) {
                Some(id) => id,
                None => return Err(elmo::Error::Misc(String::from("index does not exist"))),
            };

        fn add_one(ba: &Vec<u8>) -> Vec<u8> {
            let mut a = ba.clone();
            let mut i = a.len() - 1;
            loop {
                if a[i] == 255 {
                    a[i] = 0;
                    if i == 0 {
                        panic!("TODO handle case where add_one to binary array overflows the first byte");
                    } else {
                        i = i - 1;
                    }
                } else {
                    a[i] = a[i] + 1;
                    break;
                }
            }
            a
        }

        fn f_twok(cursor: lsm::LivingCursor, kmin: Box<[u8]>, kmax: Box<[u8]>, min_cmp: lsm::OpGt, max_cmp: lsm::OpLt) -> lsm::RangeCursor {
            let min = lsm::Min::new(kmin, min_cmp);
            let max = lsm::Max::new(kmax, max_cmp);
            let cursor = lsm::RangeCursor::new(cursor, Some(min), Some(max));
            cursor
        }

        fn f_two(preface: Vec<u8>, cursor: lsm::LivingCursor, eqvals: elmo::QueryKey, minvals: elmo::QueryKey, maxvals: elmo::QueryKey, min_cmp: lsm::OpGt, max_cmp: lsm::OpLt) -> lsm::RangeCursor {
            let mut kmin = preface.clone();
            bson::Value::push_encode_multi_for_index(&mut kmin, &eqvals, Some(&minvals));
            let mut kmax = preface;
            bson::Value::push_encode_multi_for_index(&mut kmax, &eqvals, Some(&maxvals));
            let kmin = kmin.into_boxed_slice();
            let kmax = kmax.into_boxed_slice();
            f_twok(cursor, kmin, kmax, min_cmp, max_cmp)
        }

        fn f_gt(preface: Vec<u8>, cursor: lsm::LivingCursor, vals: elmo::QueryKey, min_cmp: lsm::OpGt) -> lsm::RangeCursor {
            let mut kmin = preface.clone();
            bson::Value::push_encode_multi_for_index(&mut kmin, &vals, None);
            let kmin = kmin.into_boxed_slice();
            let min = lsm::Min::new(kmin, min_cmp);
            let cursor = lsm::RangeCursor::new(cursor, Some(min), None);
            cursor
        }

        fn f_lt(preface: Vec<u8>, cursor: lsm::LivingCursor, vals: elmo::QueryKey, max_cmp: lsm::OpLt) -> lsm::RangeCursor {
            let mut kmax = preface.clone();
            bson::Value::push_encode_multi_for_index(&mut kmax, &vals, None);
            let kmax = kmax.into_boxed_slice();
            let max = lsm::Max::new(kmax, max_cmp);
            let cursor = lsm::RangeCursor::new(cursor, None, Some(max));
            cursor
        }

        let mut key_preface = vec![];
        key_preface.push(INDEX_ENTRY);
        push_varint(&mut key_preface, collection_id);
        push_varint(&mut key_preface, index_id);

        let mut cursor =
            match bounds {
                elmo::QueryBounds::GT(vals) => f_gt(key_preface, cursor, vals, lsm::OpGt::GT),
                elmo::QueryBounds::GTE(vals) => f_gt(key_preface, cursor, vals, lsm::OpGt::GTE),
                elmo::QueryBounds::LT(vals) => f_lt(key_preface, cursor, vals, lsm::OpLt::LT),
                elmo::QueryBounds::LTE(vals) => f_lt(key_preface, cursor, vals, lsm::OpLt::LTE),
                elmo::QueryBounds::GT_LT(eqvals, minvals, maxvals) => f_two(key_preface, cursor, eqvals, minvals, maxvals, lsm::OpGt::GT, lsm::OpLt::LT),
                elmo::QueryBounds::GTE_LT(eqvals, minvals, maxvals) => f_two(key_preface, cursor, eqvals, minvals, maxvals, lsm::OpGt::GTE, lsm::OpLt::LT),
                elmo::QueryBounds::GT_LTE(eqvals, minvals, maxvals) => f_two(key_preface, cursor, eqvals, minvals, maxvals, lsm::OpGt::GT, lsm::OpLt::LTE),
                elmo::QueryBounds::GTE_LTE(eqvals, minvals, maxvals) => f_two(key_preface, cursor, eqvals, minvals, maxvals, lsm::OpGt::GTE, lsm::OpLt::LTE),
                elmo::QueryBounds::EQ(vals) => {
                    let mut kmin = key_preface.clone();
                    bson::Value::push_encode_multi_for_index(&mut kmin, &vals, None);
                    let kmax = add_one(&kmin);
                    let kmin = kmin.into_boxed_slice();
                    let kmax = kmax.into_boxed_slice();
                    f_twok(cursor, kmin, kmax, lsm::OpGt::GTE, lsm::OpLt::LT)
                },
            };

        try!(cursor.First().map_err(elmo::wrap_err));
        let seq = 
            RangeCursorVarintIterator {
                cursor: cursor,
            };

        // TODO DISTINCT problem here? we don't want this producing the same record twice

        // the iterator above yields record ids.
        // now we need something that, for each record id yielded by an
        // index entry, looks up the actual record and yields THAT.  in
        // sqlite, this was a join.

        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let seq = seq.map(
            move |record_id: Result<u64>| -> Result<elmo::Row> {
                match record_id {
                    Ok(record_id) => {
                        let mut k = vec![];
                        k.push(RECORD);
                        push_varint(&mut k, collection_id);
                        push_varint(&mut k, record_id);
                        try!(cursor.SeekRef(&lsm::KeyRef::for_slice(&k), lsm::SeekOp::SEEK_EQ).map_err(elmo::wrap_err));
                        if cursor.IsValid() {
                            let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
                            let v = try!(v.map(lsm_map_to_bson).map_err(elmo::wrap_err));
                            let v = v.into_value();
                            let row = elmo::Row {
                                doc: v,
                                pos: None,
                                score: None,
                            };
                            Ok(row)
                        } else {
                            Err(elmo::Error::Misc(String::from("record id not found?!?")))
                        }
                    },
                    Err(e) => {
                        Err(e)
                    },
                }
            });

        let rdr = 
            MyCollectionReader {
                seq: box seq,
            };

        Ok(rdr)
    }

    // TODO this could maybe return an iterator instead of a vec
    fn base_list_indexes(&self, collection_id: Option<u64>) -> Result<Vec<(u64, u64)>> {
        let cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let q = 
            match collection_id {
                Some(collection_id) => {
                    // TODO capacity
                    let mut k = vec![];
                    k.push(NAME_TO_INDEX_ID);
                    push_varint(&mut k, collection_id);
                    k.into_boxed_slice()
                },
                None => {
                    // TODO the vec! macro set capacity to match?
                    let k = vec![NAME_TO_INDEX_ID];
                    k.into_boxed_slice()
                },
            };
        let mut cursor = lsm::PrefixCursor::new(cursor, q);
        let mut a = vec![];

        try!(cursor.First().map_err(elmo::wrap_err));
        while cursor.IsValid() {
            let (collection_id, index_id) = {
                let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                let (collection_id, name) = try!(decode_key_name_to_index_id(&k));

                let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
                let index_id = try!(v.map(lsm_map_to_varint).map_err(elmo::wrap_err));

                (collection_id, index_id)
            };

            a.push((collection_id, index_id));

            try!(cursor.Next().map_err(elmo::wrap_err));
        }
        Ok(a)
    }

    fn base_list_index_infos(&self, collection_id: Option<u64>) -> Result<Vec<elmo::IndexInfo>> {
        let indexes = try!(self.base_list_indexes(collection_id));
        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let indexes = indexes.into_iter().map(
            |(collection_id, index_id)| {
                let k = encode_key_collection_id_to_properties(collection_id);
                let mut collection_properties = try!(get_value_for_key_as_bson(&mut cursor, &k)).unwrap_or(bson::Document::new());
                let db = try!(collection_properties.must_remove_string("d"));
                let coll = try!(collection_properties.must_remove_string("c"));
                //let options = try!(collection_properties.must_remove_document("o"));

                let k = encode_key_index_id_to_properties(collection_id, index_id);
                let mut index_properties = try!(get_value_for_key_as_bson(&mut cursor, &k)).unwrap_or(bson::Document::new());
                let name = try!(index_properties.must_remove_string("n"));
                let spec = try!(index_properties.must_remove_document("s"));
                let options = try!(index_properties.must_remove_document("o"));

                let info = elmo::IndexInfo {
                    db: String::from(db),
                    coll: String::from(coll),
                    name: String::from(name),
                    spec: spec,
                    options: options,
                };
                Ok(info)
            }).collect::<Result<Vec<_>>>();
        let indexes = try!(indexes);
        Ok(indexes)
    }

    fn list_indexes_for_collection_writer(&self, collection_id: u64) -> Result<Vec<MyIndexPrep>> {
        let indexes = try!(self.base_list_indexes(Some(collection_id)));
        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let indexes = indexes.into_iter().map(
            |(_, index_id)| {
                // TODO we might want to grab unique and sparse from options now, like:
                let k = encode_key_index_id_to_properties(collection_id, index_id);
                let mut index_properties = try!(get_value_for_key_as_bson(&mut cursor, &k)).unwrap_or(bson::Document::new());
                //let name = try!(index_properties.must_remove_string("n"));
                let spec = try!(index_properties.must_remove_document("s"));
                let options = try!(index_properties.must_remove_document("o"));

                let unique = 
                    match options.get("unique") {
                        Some(&bson::Value::BBoolean(b)) => b,
                        _ => false,
                    };

                let sparse = 
                    match options.get("sparse") {
                        Some(&bson::Value::BBoolean(b)) => b,
                        _ => false,
                    };
                let (normspec, weights) = try!(elmo::get_normalized_spec(&spec, &options));
                let prep = MyIndexPrep {
                    index_id: index_id,
                    spec: spec,
                    options: options,
                    normspec: normspec,
                    weights: weights,
                };
                Ok(prep)
            }).collect::<Result<Vec<_>>>();
        let indexes = try!(indexes);
        Ok(indexes)
    }

    // TODO this could maybe return an iterator instead of a vec
    fn base_list_collections(&self) -> Result<Vec<(u64, String, String)>> {
        let cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let mut cursor = lsm::PrefixCursor::new(cursor, box [NAME_TO_COLLECTION_ID]);
        let mut a = vec![];
        // TODO might need to sort by the coll name?  the sqlite version does.
        try!(cursor.First().map_err(elmo::wrap_err));
        while cursor.IsValid() {
            {
                let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                let (db, coll) = try!(decode_key_name_to_collection_id(&k));

                let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
                let collection_id = try!(v.map(lsm_map_to_varint).map_err(elmo::wrap_err));

                a.push((collection_id, db, coll));
            }
            try!(cursor.Next().map_err(elmo::wrap_err));
        }
        Ok(a)
    }

    fn base_list_collection_infos(&self) -> Result<Vec<elmo::CollectionInfo>> {
        let collections = try!(self.base_list_collections());
        let mut cursor = try!(self.conn.OpenCursor().map_err(elmo::wrap_err));
        let collections = collections.into_iter().map(
            |(collection_id, db, coll)| {
                let k = encode_key_collection_id_to_properties(collection_id);
                let mut collection_properties = try!(get_value_for_key_as_bson(&mut cursor, &k)).unwrap_or(bson::Document::new());
                //let db = try!(collection_properties.must_remove_string("d"));
                //let coll = try!(collection_properties.must_remove_string("c"));
                let options = try!(collection_properties.must_remove_document("o"));

                let info = elmo::CollectionInfo {
                    db: db,
                    coll: coll,
                    options: options,
                };
                Ok(info)
            }).collect::<Result<Vec<_>>>();
        let collections = try!(collections);
        Ok(collections)
    }

}

impl<'a> MyWriter<'a> {
    fn use_next_record_id(&mut self, collection_id: u64) -> Result<u64> {
        match self.max_record_id.entry(collection_id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let n = e.get_mut();
                *n = *n + 1;
                Ok(*n)
            },
            std::collections::hash_map::Entry::Vacant(e) => {
                let n = {
                    let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
                    // TODO capacity
                    let mut k = vec![];
                    k.push(RECORD);
                    push_varint(&mut k, collection_id + 1);
                    try!(cursor.SeekRef(&lsm::KeyRef::for_slice(&k), lsm::SeekOp::SEEK_LE).map_err(elmo::wrap_err));
                    if cursor.IsValid() {
                        let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                        if try!(k.u8_at(0).map_err(elmo::wrap_err)) == RECORD {
                            let (k_collection_id, record_id) = try!(decode_key_record(&k));
                            if collection_id == k_collection_id {
                                1 + record_id
                            } else {
                                1
                            }
                        } else {
                            1
                        }
                    } else {
                        1
                    }
                };
                e.insert(n);
                Ok(n)
            },
        }
    }

    fn use_next_collection_id(&mut self, cursor: &mut lsm::LivingCursor) -> Result<u64> {
        match self.max_collection_id {
            Some(n) => {
                let n = n + 1;
                self.max_collection_id = Some(n);
                Ok(n)
            },
            None => {
                let n = {
                    try!(cursor.SeekRef(&lsm::KeyRef::from_boxed_slice(box [COLLECTION_ID_BOUNDARY]), lsm::SeekOp::SEEK_LE).map_err(elmo::wrap_err));
                    if cursor.IsValid() {
                        let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                        if try!(k.u8_at(0).map_err(elmo::wrap_err)) == COLLECTION_ID_TO_PROPERTIES {
                            let collection_id = try!(decode_key_collection_id_to_properties(&k));
                            1 + collection_id
                        } else {
                            1
                        }
                    } else {
                        1
                    }
                };
                self.max_collection_id = Some(n);
                Ok(n)
            },
        }
    }

    fn get_collection_writer(&mut self, db: &str, coll: &str) -> Result<MyCollectionWriter> {
        let (_created, collection_id) = try!(self.base_create_collection(db, coll, bson::Document::new()));
        let indexes = try!(self.myconn.list_indexes_for_collection_writer(collection_id));
        let c = MyCollectionWriter {
            indexes: indexes,
            collection_id: collection_id,
        };
        Ok(c)
    }

    fn delete_by_prefix(&mut self, prefix: Box<[u8]>) -> Result<()> {
        // TODO it would be nice if PrefixCursor did not consume its cursor?
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));

        // TODO it would be nice if lsm had a "graveyard" delete, a way to do a
        // blind delete by prefix.

        let mut cursor = lsm::PrefixCursor::new(cursor, prefix);
        try!(cursor.First().map_err(elmo::wrap_err));
        while cursor.IsValid() {
            {
                let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                self.pending.insert(k.into_boxed_slice(), lsm::Blob::Tombstone);
            }

            try!(cursor.Next().map_err(elmo::wrap_err));
        }

        Ok(())
    }

    fn delete_by_collection_id_prefix(&mut self, tag: u8, collection_id: u64) -> Result<()> {
        let mut k = vec![];
        k.push(tag);
        push_varint(&mut k, collection_id);
        self.delete_by_prefix(k.into_boxed_slice())
    }

    fn delete_by_index_id_prefix(&mut self, tag: u8, collection_id: u64, index_id: u64) -> Result<()> {
        let mut k = vec![];
        k.push(tag);
        push_varint(&mut k, collection_id);
        push_varint(&mut k, index_id);
        self.delete_by_prefix(k.into_boxed_slice())
    }

    fn base_clear_collection(&mut self, db: &str, coll: &str) -> Result<bool> {
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        let k = encode_key_name_to_collection_id(db, coll);
        match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
            None => {
                // TODO base_created_collection checks AGAIN to see if the collection exists
                let (created, _) = try!(self.base_create_collection(db, coll, bson::Document::new()));
                Ok(created)
            },
            Some(collection_id) => {
                // all of the following tags are followed immediately by the
                // collection_id, so we can delete by prefix:

                try!(self.delete_by_collection_id_prefix(RECORD, collection_id));
                try!(self.delete_by_collection_id_prefix(INDEX_ENTRY, collection_id));
                try!(self.delete_by_collection_id_prefix(RECORD_ID_TO_INDEX_ENTRY, collection_id));

                Ok(false)
            },
        }
    }

    fn create_index(&mut self, info: elmo::IndexInfo) -> Result<bool> {
        //println!("create_index: {:?}", info);
        let (_created, collection_id) = try!(self.base_create_collection(&info.db, &info.coll, bson::Document::new()));
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        let k = encode_key_name_to_index_id(collection_id, &info.name);
        match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
            Some(index_id) => {
                let k = encode_key_index_id_to_properties(collection_id, index_id);
                let mut index_properties = try!(get_value_for_key_as_bson(&mut cursor, &k)).unwrap_or(bson::Document::new());
                let name = try!(index_properties.must_remove_string("n"));
                let spec = try!(index_properties.must_remove_document("s"));
                let options = try!(index_properties.must_remove_document("o"));
                if spec != info.spec {
                    // note that we do not compare the options.
                    // I think mongo does it this way too.
                    Err(elmo::Error::Misc(String::from("index already exists with different keys")))
                } else {
                    Ok(false)
                }
            },
            None => {
                // TODO next index id for this collection
                let index_id = 2;
                self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(u64_to_boxed_varint(index_id)));

                // now create entries for all the existing records

                let unique = 
                    match info.options.get("unique") {
                        Some(&bson::Value::BBoolean(b)) => b,
                        _ => false,
                    };
                let (normspec, weights) = try!(elmo::get_normalized_spec(&info.spec, &info.options));

                let mut k = vec![];
                k.push(RECORD);
                push_varint(&mut k, collection_id);
                let mut cursor = lsm::PrefixCursor::new(cursor, k.into_boxed_slice());
                try!(cursor.First().map_err(elmo::wrap_err));
                while cursor.IsValid() {
                    {
                        let k = try!(cursor.KeyRef().map_err(elmo::wrap_err));
                        let (_, record_id) = try!(decode_key_record(&k));
                        let ba_record_id = u64_to_boxed_varint(record_id);
                        let v = try!(cursor.LiveValueRef().map_err(elmo::wrap_err));
                        let v = try!(v.map(lsm_map_to_bson).map_err(elmo::wrap_err));
                        let entries = try!(elmo::get_index_entries(&v, &normspec, &weights, &info.options));
                        let ba_collection_id = u64_to_boxed_varint(collection_id);
                        let ba_index_id = u64_to_boxed_varint(index_id);
                        for vals in entries {
                            try!(self.add_index_entry(&ba_collection_id, &ba_index_id, &ba_record_id, vals, unique));
                        }
                    }

                    try!(cursor.Next().map_err(elmo::wrap_err));
                }

                // now store the index id to properties

                let k = encode_key_index_id_to_properties(collection_id, index_id);
                let mut properties = bson::Document::new();
                properties.set_string("n", info.name);
                properties.set_document("s", info.spec);
                properties.set_document("o", info.options);
                self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(properties.to_bson_array().into_boxed_slice()));

                Ok(true)
            }
        }
    }

    fn base_create_indexes(&mut self, what: Vec<elmo::IndexInfo>) -> Result<Vec<bool>> {
        let mut v = Vec::new();
        for info in what {
            let b = try!(self.create_index(info));
            v.push(b);
        }
        Ok(v)
    }

    fn base_drop_collection(&mut self, db: &str, coll: &str) -> Result<bool> {
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        let k = encode_key_name_to_collection_id(db, coll);
        match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
            None => Ok(false),
            Some(collection_id) => {
                self.pending.insert(k, lsm::Blob::Tombstone);
 
                // all of the following tags are followed immediately by the
                // collection_id, so we can delete by prefix:

                try!(self.delete_by_collection_id_prefix(COLLECTION_ID_TO_PROPERTIES, collection_id));
                try!(self.delete_by_collection_id_prefix(NAME_TO_INDEX_ID, collection_id));
                try!(self.delete_by_collection_id_prefix(INDEX_ID_TO_PROPERTIES, collection_id));
                try!(self.delete_by_collection_id_prefix(RECORD, collection_id));
                try!(self.delete_by_collection_id_prefix(INDEX_ENTRY, collection_id));
                try!(self.delete_by_collection_id_prefix(RECORD_ID_TO_INDEX_ENTRY, collection_id));

                Ok(true)
            },
        }
    }

    fn base_drop_database(&mut self, db_to_delete: &str) -> Result<bool> {
        let mut b = false;
        for (_, db, coll) in try!(self.myconn.base_list_collections()) {
            if db == db_to_delete {
                try!(self.base_drop_collection(&db, &coll));
                b = true;
            }
        }
        Ok(b)
    }

    fn base_drop_index(&mut self, db: &str, coll: &str, name: &str) -> Result<bool> {
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        match try!(get_value_for_key_as_varint(&mut cursor, &encode_key_name_to_collection_id(&db, &coll))) {
            None => Ok(false),
            Some(collection_id) => {
                let k = encode_key_name_to_index_id(collection_id, name);
                match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
                    None => Ok(false),
                    Some(index_id) => {
                        self.pending.insert(k.into_boxed_slice(), lsm::Blob::Tombstone);

                        try!(self.delete_by_index_id_prefix(INDEX_ID_TO_PROPERTIES, collection_id, index_id));
                        try!(self.delete_by_index_id_prefix(INDEX_ENTRY, collection_id, index_id));
                        try!(self.delete_by_index_id_prefix(RECORD_ID_TO_INDEX_ENTRY, collection_id, index_id));

                        Ok(true)
                    },
                }
            },
        }
    }

    fn base_create_collection(&mut self, db: &str, coll: &str, options: bson::Document) -> Result<(bool, u64)> {
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        let k = encode_key_name_to_collection_id(db, coll);
        match try!(get_value_for_key_as_varint(&mut cursor, &k)) {
            Some(id) => Ok((false, id)),
            None => {
                let collection_id = try!(self.use_next_collection_id(&mut cursor));
                self.pending.insert(k, lsm::Blob::Array(u64_to_boxed_varint(collection_id)));

                // create mongo index for _id
                match options.get("autoIndexId") {
                    Some(&bson::Value::BBoolean(false)) => {
                    },
                    _ => {
                        let index_id = PRIMARY_INDEX_ID;
                        let k = encode_key_name_to_index_id(collection_id, "_id_");
                        self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(u64_to_boxed_varint(index_id)));

                        let k = encode_key_index_id_to_properties(collection_id, index_id);
                        let mut properties = bson::Document::new();
                        properties.set_str("n", "_id_");
                        let spec = bson::Document {pairs: vec![(String::from("_id"), bson::Value::BInt32(1))]};
                        let options = bson::Document {pairs: vec![(String::from("unique"), bson::Value::BBoolean(true))]};
                        properties.set_document("s", spec);
                        properties.set_document("o", options);
                        self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(properties.to_bson_array().into_boxed_slice()));
                    },
                }

                let k = encode_key_collection_id_to_properties(collection_id);
                let mut properties = bson::Document::new();
                properties.set_str("d", db);
                properties.set_str("c", coll);
                properties.set_document("o", options);
                self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(properties.to_bson_array().into_boxed_slice()));

                Ok((true, collection_id))
            },
        }
    }

    fn update_indexes_delete(&mut self, indexes: &Vec<MyIndexPrep>, ba_collection_id: &Box<[u8]>, ba_record_id: &Box<[u8]>) -> Result<()> {
        for ndx in indexes {
            // TODO delete all index entries (and their back links) which involve this record_id.
            // this *could* be done by simply iterating over all the index entries,
            // unpacking each one, seeing if the record id matches, and remove it if so, etc.
            // back links make it faster when the index is large.
        }
        Ok(())
    }

    fn add_index_entry(&mut self, ba_collection_id: &Box<[u8]>, ba_index_id: &Box<[u8]>, ba_record_id: &Box<[u8]>, vals: Vec<(bson::Value, bool)>, unique: bool) -> Result<()> {
        let vref = vals.iter().map(|&(ref v,neg)| (v,neg)).collect::<Vec<_>>();
        let k = bson::Value::encode_multi_for_index(&vref, None);
        // TODO capacity
        let mut index_entry = vec![];
        index_entry.push(INDEX_ENTRY);
        index_entry.push_all(ba_collection_id);
        index_entry.push_all(ba_index_id);
        index_entry.push_all(&k);
        if !unique {
            index_entry.push_all(&ba_record_id);
        }

        // do the backward entry first, because the other one takes ownership
        let mut backward_entry = vec![];
        backward_entry.clear();
        backward_entry.push(RECORD_ID_TO_INDEX_ENTRY);
        backward_entry.push_all(ba_collection_id);
        backward_entry.push_all(&ba_index_id);
        backward_entry.push_all(ba_record_id);
        backward_entry.push_all(&index_entry);
        self.pending.insert(backward_entry.into_boxed_slice(), lsm::Blob::Array(box []));

        // now the index entry itself, since ownership is transferred
        self.pending.insert(index_entry.into_boxed_slice(), lsm::Blob::Array(ba_record_id.clone()));

        Ok(())
    }

    fn update_indexes_insert(&mut self, indexes: &Vec<MyIndexPrep>, ba_collection_id: &Box<[u8]>, ba_record_id: &Box<[u8]>, v: &bson::Document) -> Result<()> {
        for ndx in indexes {
            let entries = try!(elmo::get_index_entries(&v, &ndx.normspec, &ndx.weights, &ndx.options));
            // TODO don't look this up here.  store it in the cached info.
            let unique = 
                match ndx.options.get("unique") {
                    Some(&bson::Value::BBoolean(b)) => b,
                    _ => false,
                };
            // TODO store this in the cache?
            let ba_index_id = u64_to_boxed_varint(ndx.index_id);
            for vals in entries {
                try!(self.add_index_entry(ba_collection_id, &ba_index_id, ba_record_id, vals, unique));
            }
        }
        Ok(())
    }

}

impl<'a> elmo::StorageWriter for MyWriter<'a> {
    fn update(&mut self, db: &str, coll: &str, v: &bson::Document) -> Result<()> {
        match v.get("_id") {
            None => Err(elmo::Error::Misc(String::from("cannot update without _id"))),
            Some(id) => {
                let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
                // TODO maybe we should pass the cursor to get_collection_writer()
                let cw = try!(self.get_collection_writer(db, coll));
                let record_id = try!(find_record(&mut cursor, cw.collection_id, &id));

                // TODO capacity
                let mut k = vec![];
                k.push(RECORD);
                let ba_collection_id = u64_to_boxed_varint(cw.collection_id);
                k.push_all(&ba_collection_id);
                let ba_record_id = u64_to_boxed_varint(record_id);
                k.push_all(&ba_record_id);
                self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(v.to_bson_array().into_boxed_slice()));

                try!(self.update_indexes_delete(&cw.indexes, &ba_collection_id, &ba_record_id));
                try!(self.update_indexes_insert(&cw.indexes, &ba_collection_id, &ba_record_id, v));

                Ok(())
            },
        }
    }

    fn delete(&mut self, db: &str, coll: &str, id: &bson::Value) -> Result<bool> {
        let mut cursor = try!(self.myconn.conn.OpenCursor().map_err(elmo::wrap_err));
        // TODO maybe we should pass the cursor to get_collection_writer()
        let cw = try!(self.get_collection_writer(db, coll));
        let record_id = try!(find_record(&mut cursor, cw.collection_id, &id));

        // TODO capacity
        let mut k = vec![];
        k.push(RECORD);
        let ba_collection_id = u64_to_boxed_varint(cw.collection_id);
        k.push_all(&ba_collection_id);
        let ba_record_id = u64_to_boxed_varint(record_id);
        k.push_all(&ba_record_id);
        self.pending.insert(k.into_boxed_slice(), lsm::Blob::Tombstone);

        try!(self.update_indexes_delete(&cw.indexes, &ba_collection_id, &ba_record_id));

        // TODO ouch.  mongo wants us to return whether we deleted this or not,
        // but this could be a blind write, which is much faster.
        // for now, we lie and say true.

        let deleted = true;

        Ok(deleted)
    }

    fn insert(&mut self, db: &str, coll: &str, v: &bson::Document) -> Result<()> {
        let cw = try!(self.get_collection_writer(db, coll));
        // TODO capacity
        let mut k = vec![];
        k.push(RECORD);
        let ba_collection_id = u64_to_boxed_varint(cw.collection_id);
        k.push_all(&ba_collection_id);
        let record_id = try!(self.use_next_record_id(cw.collection_id));
        let ba_record_id = u64_to_boxed_varint(record_id);
        k.push_all(&ba_record_id);
        self.pending.insert(k.into_boxed_slice(), lsm::Blob::Array(v.to_bson_array().into_boxed_slice()));

        try!(self.update_indexes_insert(&cw.indexes, &ba_collection_id, &ba_record_id, v));

        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<()> {
        if !self.pending.is_empty() {
            let pending = std::mem::replace(&mut self.pending, HashMap::new());
            let g = try!(self.myconn.conn.WriteSegment2(pending).map_err(elmo::wrap_err));
            try!(self.tx.commitSegments(vec![g]).map_err(elmo::wrap_err));
        }
        Ok(())
    }

    fn rollback(mut self: Box<Self>) -> Result<()> {
        // since we haven't been writing segments, do nothing here
        Ok(())
    }

    fn create_collection(&mut self, db: &str, coll: &str, options: bson::Document) -> Result<bool> {
        let (created, _collection_id) = try!(self.base_create_collection(db, coll, options));
        Ok(created)
    }

    fn drop_collection(&mut self, db: &str, coll: &str) -> Result<bool> {
        self.base_drop_collection(db, coll)
    }

    fn create_indexes(&mut self, what: Vec<elmo::IndexInfo>) -> Result<Vec<bool>> {
        self.base_create_indexes(what)
    }

    fn rename_collection(&mut self, old_name: &str, new_name: &str, drop_target: bool) -> Result<bool> {
        unimplemented!();
    }

    fn drop_index(&mut self, db: &str, coll: &str, name: &str) -> Result<bool> {
        self.base_drop_index(db, coll, name)
    }

    fn drop_database(&mut self, db: &str) -> Result<bool> {
        self.base_drop_database(db)
    }

    fn clear_collection(&mut self, db: &str, coll: &str) -> Result<bool> {
        self.base_clear_collection(db, coll)
    }

}

// TODO do we need to declare that StorageWriter must implement Drop ?
impl<'a> Drop for MyWriter<'a> {
    fn drop(&mut self) {
        // TODO rollback
    }
}

// TODO do we need to declare that StorageReader must implement Drop ?
impl Drop for MyReader {
    fn drop(&mut self) {
    }
}

impl Drop for MyCollectionReader {
    fn drop(&mut self) {
    }
}

impl Iterator for MyCollectionReader {
    type Item = Result<elmo::Row>;
    fn next(&mut self) -> Option<Self::Item> {
        self.seq.next()
    }
}

impl elmo::StorageBase for MyReader {
    fn get_reader_collection_scan(&self, db: &str, coll: &str) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_collection_scan(db, coll));
        Ok(box rdr)
    }

    fn get_reader_text_index_scan(&self, ndx: &elmo::IndexInfo, eq: elmo::QueryKey, terms: Vec<elmo::TextQueryTerm>) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_text_index_scan(self.myconn.clone(), false, ndx, eq, terms));
        Ok(box rdr)
    }

    fn get_reader_regular_index_scan(&self, ndx: &elmo::IndexInfo, bounds: elmo::QueryBounds) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_regular_index_scan(ndx, bounds));
        Ok(box rdr)
    }

    fn list_collections(&self) -> Result<Vec<elmo::CollectionInfo>> {
        self.myconn.base_list_collection_infos()
    }

    fn list_indexes(&self) -> Result<Vec<elmo::IndexInfo>> {
        self.myconn.base_list_index_infos(None)
    }

}

impl elmo::StorageReader for MyReader {
    fn into_reader_collection_scan(mut self: Box<Self>, db: &str, coll: &str) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_collection_scan(db, coll));
        Ok(box rdr)
    }

    fn into_reader_text_index_scan(&self, ndx: &elmo::IndexInfo, eq: elmo::QueryKey, terms: Vec<elmo::TextQueryTerm>) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_text_index_scan(self.myconn.clone(), true, ndx, eq, terms));
        Ok(box rdr)
    }

    fn into_reader_regular_index_scan(&self, ndx: &elmo::IndexInfo, bounds: elmo::QueryBounds) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_regular_index_scan(ndx, bounds));
        Ok(box rdr)
    }

}

impl<'a> elmo::StorageBase for MyWriter<'a> {
    fn get_reader_collection_scan(&self, db: &str, coll: &str) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_collection_scan(db, coll));
        Ok(box rdr)
    }

    fn get_reader_text_index_scan(&self, ndx: &elmo::IndexInfo, eq: elmo::QueryKey, terms: Vec<elmo::TextQueryTerm>) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_text_index_scan(self.myconn.clone(), false, ndx, eq, terms));
        Ok(box rdr)
    }

    fn get_reader_regular_index_scan(&self, ndx: &elmo::IndexInfo, bounds: elmo::QueryBounds) -> Result<Box<Iterator<Item=Result<elmo::Row>> + 'static>> {
        let rdr = try!(self.myconn.get_reader_regular_index_scan(ndx, bounds));
        Ok(box rdr)
    }

    fn list_collections(&self) -> Result<Vec<elmo::CollectionInfo>> {
        self.myconn.base_list_collection_infos()
    }

    fn list_indexes(&self) -> Result<Vec<elmo::IndexInfo>> {
        self.myconn.base_list_index_infos(None)
    }

}

impl elmo::StorageConnection for MyPublicConn {
    fn begin_write<'a>(&'a self) -> Result<Box<elmo::StorageWriter + 'a>> {
        let tx = try!(self.myconn.conn.GetWriteLock().map_err(elmo::wrap_err));

        let w = MyWriter {
            myconn: self.myconn.clone(),
            tx: tx,
            pending: HashMap::new(),
            max_collection_id: None,
            max_record_id: HashMap::new(),
        };
        Ok(box w)
    }

    fn begin_read(&self) -> Result<Box<elmo::StorageReader + 'static>> {
        let r = MyReader {
            myconn: self.myconn.clone(),
        };
        Ok(box r)
    }
}

fn base_connect(name: &str) -> lsm::Result<lsm::db> {
    lsm::db::new(String::from(name), lsm::DEFAULT_SETTINGS)
}

pub fn connect(name: &str) -> Result<Box<elmo::StorageConnection>> {
    let conn = try!(base_connect(name).map_err(elmo::wrap_err));
    let c = MyConn {
        conn: conn,
    };
    let c = MyPublicConn {
        myconn: std::rc::Rc::new(c)
    };
    Ok(box c)
}

#[derive(Clone)]
pub struct MyFactory {
    filename: String,
}

impl MyFactory {
    pub fn new(filename: String) -> MyFactory {
        MyFactory {
            filename: filename,
        }
    }
}

impl elmo::ConnectionFactory for MyFactory {
    fn open(&self) -> elmo::Result<elmo::Connection> {
        let conn = try!(connect(&self.filename));
        let conn = elmo::Connection::new(conn);
        Ok(conn)
    }

    fn clone_for_new_thread(&self) -> Box<elmo::ConnectionFactory + Send> {
        box self.clone()
    }
}

