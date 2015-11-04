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
#![feature(convert)]
#![feature(associated_consts)]
#![feature(vec_push_all)]
#![feature(clone_from_slice)]
#![feature(drain)]
#![feature(iter_arith)]

// TODO turn the following warnings back on later
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

extern crate misc;

use misc::endian;
use misc::varint;
use misc::Lend;

use std::sync::mpsc;
use std::thread;

use std::io;
use std::io::Seek;
use std::io::Read;
use std::io::Write;
use std::io::SeekFrom;
use std::cmp::Ordering;
use std::fs::File;
use std::fs::OpenOptions;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

const SIZE_32: usize = 4; // like std::mem::size_of::<u32>()
const SIZE_16: usize = 2; // like std::mem::size_of::<u16>()

// all new segments start at level 0.
//
// level 0 size limit is 0 because it is unused, because promotion
// out of level 0 is not based on a size threshold.
//
// final level size limit is 0 because it is unused, because nothing ever
// gets promoted out of the last level.
const LEVEL_SIZE_LIMITS_IN_KB: [u64; 4] = [0, 400, 40000, 0];

pub type PageNum = u32;
pub type PageCount = u32;
// type PageSize = u32;

// TODO also perhaps the type representing size of a value, u32
// size of a value should NOT be usize, right?

// TODO there is code which assumes that PageNum is u32.
// but that's the nature of the file format.  the type alias
// isn't so much so that we can change it, but rather, to make
// reading the code easier.

pub enum Blob {
    Stream(Box<Read>),
    Array(Box<[u8]>),
    Tombstone,
    // TODO Graveyard?  (considered a delete of anything with the prefix?)

    // TODO consider having a way here to represent an overflow so 
    // that it can be moved without copying it during a merge
}

impl Blob {
    fn is_tombstone(&self) -> bool {
        match self {
            &Blob::Tombstone => true,
            &Blob::Stream(_) => false,
            &Blob::Array(_) => false,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    // TODO remove Misc
    Misc(String),

    // TODO more detail within CorruptFile
    CorruptFile(&'static str),

    Io(std::io::Error),
    Utf8(std::str::Utf8Error),

    CursorNotValid,
    InvalidPageNumber,
    InvalidPageType(u8),
    RootPageNotInSegmentBlockList,
    Poisoned,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            Error::Io(ref err) => write!(f, "IO error: {}", err),
            Error::Utf8(ref err) => write!(f, "Utf8 error: {}", err),
            Error::Misc(ref s) => write!(f, "Misc error: {}", s),
            Error::CorruptFile(s) => write!(f, "Corrupt file: {}", s),
            Error::Poisoned => write!(f, "Poisoned"),
            Error::CursorNotValid => write!(f, "Cursor not valid"),
            Error::InvalidPageNumber => write!(f, "Invalid page number"),
            Error::InvalidPageType(b) => write!(f, "Invalid page type: {}", b),
            Error::RootPageNotInSegmentBlockList => write!(f, "Root page not in segment block list"),
        }
    }
}

impl std::error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Io(ref err) => std::error::Error::description(err),
            Error::Utf8(ref err) => std::error::Error::description(err),
            Error::Misc(ref s) => s.as_str(),
            Error::CorruptFile(s) => s,
            Error::Poisoned => "poisoned",
            Error::CursorNotValid => "cursor not valid",
            Error::InvalidPageNumber => "invalid page number",
            Error::InvalidPageType(b) => "invalid page type",
            Error::RootPageNotInSegmentBlockList => "Root page not in segment block list",
        }
    }

    // TODO cause
}

pub fn wrap_err<E: std::error::Error + 'static>(err: E) -> Error {
    Error::Misc(format!("{}", err))
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(err: std::str::Utf8Error) -> Error {
        Error::Utf8(err)
    }
}

impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(_err: std::sync::PoisonError<T>) -> Error {
        Error::Poisoned
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum BlockRequest {
    Any,
    MinimumSize(PageCount),
    StartOrAfterMinimumSize(Vec<PageNum>, PageNum, PageCount),
    StartOrAny(Vec<PageNum>),
}

impl BlockRequest {
    fn start_is(&self, pg: PageNum) -> bool {
        match self {
            &BlockRequest::Any => false,
            &BlockRequest::MinimumSize(_) => false,

            &BlockRequest::StartOrAny(ref v) => v.iter().any(|n| *n == pg),
            &BlockRequest::StartOrAfterMinimumSize(ref v, _, _) => v.iter().any(|n| *n == pg),
        }
    }

    fn get_after(&self) -> Option<PageNum> {
        match self {
            &BlockRequest::Any => None,
            &BlockRequest::MinimumSize(_) => None,

            &BlockRequest::StartOrAny(_) => None,
            &BlockRequest::StartOrAfterMinimumSize(_, after, _) => Some(after),
        }
    }
}

#[derive(Debug)]
pub enum MergePromotionRule {
    Promote,
    Stay,
    Threshold(PageCount),
}

// kvp is the struct used to provide key-value pairs downward,
// for storage into the database.
struct kvp {
    Key: Box<[u8]>,
    Value: Blob,
}

#[derive(Debug,Clone)]
pub struct BlockList {
    blocks: Vec<PageBlock>,
}

impl BlockList {
    fn new() -> Self {
        BlockList {
            blocks: vec![],
        }
    }

    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn count_pages(&self) -> PageCount {
        self.blocks.iter().map(|pb| pb.count_pages()).sum()
    }

    fn last_page(&self) -> PageNum {
        // TODO assume it is sorted
        // TODO assuming self.blocks is not empty
        self.blocks[self.blocks.len() - 1].lastPage
    }

    fn first_page(&self) -> PageNum {
        // TODO assume it is sorted
        // TODO assuming self.blocks is not empty
        self.blocks[0].firstPage
    }

    fn remove_block(&mut self, i: usize) -> PageBlock {
        self.blocks.remove(i)
    }

    fn contains_page(&self, pgnum: PageNum) -> bool {
        for blk in self.blocks.iter() {
            if blk.contains_page(pgnum) {
                return true;
            }
        }
        return false;
    }

    fn would_extend_an_existing_block(&self, pg: PageNum) -> bool {
        for i in 0 .. self.blocks.len() {
            if self.blocks[i].lastPage + 1 == pg {
                return true;
            }
        }
        false
    }

    fn add_page_no_reorder(&mut self, pg: PageNum) -> bool {
        for i in 0 .. self.blocks.len() {
            if self.blocks[i].lastPage + 1 == pg {
                self.blocks[i].lastPage = pg;
                return true;
            }
        }

        let b = PageBlock::new(pg, pg);
        self.blocks.push(b);
        return false;
    }

    fn add_block_no_reorder(&mut self, blk: PageBlock) -> bool {
        for i in 0 .. self.blocks.len() {
            if self.blocks[i].lastPage + 1 == blk.firstPage {
                self.blocks[i].lastPage = blk.lastPage;
                return true;
            }
        }

        self.blocks.push(blk);
        return false;
    }

    fn invert(&mut self) {
        let len = self.blocks.len();
        self.blocks.sort_by(|a,b| a.firstPage.cmp(&b.firstPage));
        for i in 0 .. len - 1 {
            self.blocks[i].firstPage = self.blocks[i].lastPage + 1;
            self.blocks[i].lastPage = self.blocks[i + 1].firstPage - 1;
            assert!(self.blocks[i].firstPage <= self.blocks[i].lastPage);
        }
        // this function finds the space between the blocks.
        // the last block didn't get touched, and is still a block in use.
        // so remove it.
        self.blocks.remove(len - 1);
    }

    fn add_blocklist_no_reorder(&mut self, other: &BlockList) {
        // TODO call add_block_no_reorder, and return a bool
        for blk in other.blocks.iter() {
            self.blocks.push(blk.clone());
        }
    }

    fn sort_and_consolidate(&mut self) {
        self.blocks.sort_by(|a,b| a.firstPage.cmp(&b.firstPage));
        if cfg!(expensive_check) 
        {
            for i in 1 .. self.blocks.len() {
                assert!(self.blocks[i].firstPage > self.blocks[i - 1].lastPage);
            }
        }
        loop {
            if self.blocks.len()==1 {
                break;
            }
            let mut did = false;
            for i in 1 .. self.blocks.len() {
                if self.blocks[i - 1].lastPage + 1 == self.blocks[i].firstPage {
                    self.blocks[i - 1].lastPage = self.blocks[i].lastPage;
                    self.blocks.remove(i);
                    did = true;
                    break;
                }
            }
            if !did {
                break;
            }
        }
        // TODO the list is still sorted, right?
    }

    fn sort_by_size_desc_page_asc(&mut self) {
        self.blocks.sort_by(
            |a,b| 
                match b.count_pages().cmp(&a.count_pages()) {
                    Ordering::Equal => a.firstPage.cmp(&b.firstPage),
                    c => c,
                }
                );
    }

    fn encode(&self, pb: &mut Vec<u8>) {
        // we store each PageBlock as first/offset instead of first/last, since the
        // offset will always compress better as a varint.
        
        // if there are multiple blocks, we store the firstPage field
        // of all blocks after the first one as offsets from th first one.
        // this requires that the list was sorted before this func was called.

        misc::push_varint(pb, self.blocks.len() as u64);
        if self.blocks.len() > 0 {
            let first_page = self.blocks[0].firstPage;
            misc::push_varint(pb, self.blocks[0].firstPage as u64);
            misc::push_varint(pb, (self.blocks[0].lastPage - self.blocks[0].firstPage) as u64);
            if self.blocks.len() > 1 {
                for i in 1 .. self.blocks.len() {
                    assert!(self.blocks[i].firstPage > first_page);
                    misc::push_varint(pb, (self.blocks[i].firstPage - first_page) as u64);
                    misc::push_varint(pb, (self.blocks[i].lastPage - self.blocks[i].firstPage) as u64);
                }
            }
        }
    }

    fn encoded_len(&self) -> usize {
        // TODO do this without mem alloc
        let mut v = vec![];
        self.encode(&mut v);
        v.len()
    }

    fn read(pr: &[u8], cur: &mut usize) -> Self {
        let count_blocks = varint::read(pr, cur) as usize;
        let mut list = BlockList {
            blocks: Vec::with_capacity(count_blocks),
        };
        for i in 0 .. count_blocks {
            let first_page = varint::read(pr, cur) as PageNum
                +
                if i > 0 {
                    list.blocks[0].firstPage
                } else {
                    0
                };
            let offset = varint::read(pr, cur) as PageNum;
            let b = PageBlock::new(first_page, first_page + offset);
            list.blocks.push(b);
        }
        list
    }

    fn skip(pr: &[u8], cur: &mut usize) {
        // TODO do this without mem alloc
        let unused = Self::read(pr, cur);
    }
}

impl std::ops::Index<usize> for BlockList {
    type Output = PageBlock;

    fn index<'a>(&'a self, i: usize) -> &'a PageBlock {
        &self.blocks[i]
    }
}

fn get_level_size(i: usize) -> u64 {
    let mut n = 1;
    for _ in 0 .. i+1 {
        n *= 10;
    }
    n
}

fn split3<T>(a: &mut [T], i: usize) -> (&mut [T], &mut [T], &mut [T]) {
    let (before, a2) = a.split_at_mut(i);
    let (islice, after) = a2.split_at_mut(1);
    (before, islice, after)
}

pub enum KeyRef<'a> {
    Boxed(Box<[u8]>),
    // TODO consider a type representing an overflow reference?

    // the other two are references into the page
    Prefixed(&'a [u8],&'a [u8]),
    Array(&'a [u8]),
}

impl<'a> std::fmt::Debug for KeyRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match *self {
            KeyRef::Boxed(ref a) => write!(f, "Boxed, a={:?}", a),
            KeyRef::Prefixed(front,back) => write!(f, "Prefixed, front={:?}, back={:?}", front, back),
            KeyRef::Array(a) => write!(f, "Array, val={:?}", a),
        }
    }
}

impl<'a> KeyRef<'a> {
    pub fn len(&self) -> usize {
        match *self {
            KeyRef::Boxed(ref a) => a.len(),
            KeyRef::Array(a) => a.len(),
            KeyRef::Prefixed(front, back) => front.len() + back.len(),
        }
    }

    pub fn u8_at(&self, i: usize) -> Result<u8> {
        match self {
            &KeyRef::Boxed(ref a) => {
                if i < a.len() {
                    Ok(a[i])
                } else {
                    Err(Error::Misc(String::from("u8_at: out of range 1")))
                }
            },
            &KeyRef::Array(a) => {
                if i < a.len() {
                    Ok(a[i])
                } else {
                    Err(Error::Misc(String::from("u8_at: out of range 2")))
                }
            },
            &KeyRef::Prefixed(front, back) => {
                if i < front.len() {
                    Ok(front[i])
                } else {
                    let i = i - front.len();
                    if i < back.len() {
                        Ok(back[i])
                    } else {
                        Err(Error::Misc(String::from("u8_at: out of range 3")))
                    }
                }
            },
        }
    }

    pub fn compare_with(&self, k: &[u8]) -> Ordering {
        match self {
            &KeyRef::Boxed(ref a) => {
                bcmp::Compare(&a, &k)
            },
            &KeyRef::Array(a) => {
                bcmp::Compare(&a, &k)
            },
            &KeyRef::Prefixed(front, back) => {
                Self::compare_px_y(front, back, k)
            },
        }
    }

    pub fn starts_with(&self, k: &[u8]) -> bool {
        if self.len() < k.len() {
            return false;
        } else {
            match self {
                &KeyRef::Boxed(ref a) => {
                    k == &a[0 .. k.len()]
                },
                &KeyRef::Array(a) => {
                    k == &a[0 .. k.len()]
                },
                &KeyRef::Prefixed(front, back) => {
                    if k.len() <= front.len() {
                        k == &front[0 .. k.len()]
                    } else {
                        &k[0 .. front.len()] == &front[0 .. front.len()]
                        && 
                        &k[front.len() ..] == &back[0 .. k.len() - front.len()]
                    }
                },
            }
        }
    }

    /// A KeyRef is conceptually just a bunch of bytes, but it can be represented in
    /// three different ways, depending on whether the key in the page was overflowed
    /// or prefixed or neither.  This function accepts a func which is to be applied
    /// to a range of the key.  Whenever possible, the func is called without allocation.
    /// But if the requested range crosses the front/back boundary of a prefixed key,
    /// then an alloc+copy will be needed.
    pub fn map_range<T, F: Fn(&[u8]) -> Result<T>>(&self, begin: usize, end: usize, func: F) -> Result<T> {
        if end <= begin {
            return Err(Error::Misc(String::from("illegal range")));
        }
        match self {
            &KeyRef::Array(a) => {
                if end <= a.len() {
                    let t = try!(func(&a[begin .. end]));
                    Ok(t)
                } else {
                    Err(Error::Misc(String::from("map_range: out of range 1")))
                }
            },
            &KeyRef::Boxed(ref a) => {
                if end <= a.len() {
                    let t = try!(func(&a[begin .. end]));
                    Ok(t)
                } else {
                    Err(Error::Misc(String::from("map_range: out of range 2")))
                }
            },
            &KeyRef::Prefixed(front, back) => {
                if end <= front.len() {
                    let t = try!(func(&front[begin .. end]));
                    Ok(t)
                } else if begin >= front.len() {
                    let begin = begin - front.len();
                    let end = end - front.len();
                    if end <= back.len() {
                        let t = try!(func(&back[begin .. end]));
                        Ok(t)
                    } else {
                        Err(Error::Misc(String::from("map_range: out of range 3")))
                    }
                } else {
                    // the range we want is split across the front and back.
                    // in this case we have to alloc.
                    let mut a = Vec::with_capacity(end - begin);
                    a.push_all(&front[begin .. ]);
                    let end = end - front.len();
                    if end <= back.len() {
                        a.push_all(&back[0 .. end]);
                        let t = try!(func(&a));
                        Ok(t)
                    } else {
                        Err(Error::Misc(String::from("map_range: out of range 4")))
                    }
                }
            },
        }
    }

    pub fn from_boxed_slice(k: Box<[u8]>) -> KeyRef<'a> {
        KeyRef::Boxed(k)
    }

    pub fn for_slice(k: &[u8]) -> KeyRef {
        KeyRef::Array(k)
    }

    pub fn into_boxed_slice(self) -> Box<[u8]> {
        match self {
            KeyRef::Boxed(a) => {
                a
            },
            KeyRef::Array(a) => {
                let mut k = Vec::with_capacity(a.len());
                k.push_all(a);
                k.into_boxed_slice()
            },
            KeyRef::Prefixed(front,back) => {
                let mut k = Vec::with_capacity(front.len() + back.len());
                k.push_all(front);
                k.push_all(back);
                k.into_boxed_slice()
            },
        }
    }

    // TODO move this to the bcmp module?
    fn compare_px_py(px: &[u8], x: &[u8], py: &[u8], y: &[u8]) -> Ordering {
        let xlen = px.len() + x.len();
        let ylen = py.len() + y.len();
        let len = std::cmp::min(xlen, ylen);
        for i in 0 .. len {
            let xval = 
                if i<px.len() {
                    px[i]
                } else {
                    x[i - px.len()]
                };
            let yval = 
                if i<py.len() {
                    py[i]
                } else {
                    y[i - py.len()]
                };
            let c = xval.cmp(&yval);
            if c != Ordering::Equal {
                return c;
            }
        }
        return xlen.cmp(&ylen);
    }

    // TODO move this to the bcmp module?
    fn compare_px_y(px: &[u8], x: &[u8], y: &[u8]) -> Ordering {
        let xlen = px.len() + x.len();
        let ylen = y.len();
        let len = std::cmp::min(xlen, ylen);
        for i in 0 .. len {
            let xval = 
                if i<px.len() {
                    px[i]
                } else {
                    x[i - px.len()]
                };
            let yval = y[i];
            let c = xval.cmp(&yval);
            if c != Ordering::Equal {
                return c;
            }
        }
        return xlen.cmp(&ylen);
    }

    // TODO move this to the bcmp module?
    fn compare_x_py(x: &[u8], py: &[u8], y: &[u8]) -> Ordering {
        let xlen = x.len();
        let ylen = py.len() + y.len();
        let len = std::cmp::min(xlen, ylen);
        for i in 0 .. len {
            let xval = x[i];
            let yval = 
                if i<py.len() {
                    py[i]
                } else {
                    y[i - py.len()]
                };
            let c = xval.cmp(&yval);
            if c != Ordering::Equal {
                return c;
            }
        }
        return xlen.cmp(&ylen);
    }

    pub fn cmp(x: &KeyRef, y: &KeyRef) -> Ordering {
        match (x,y) {
            (&KeyRef::Boxed(ref x_k), &KeyRef::Boxed(ref y_k)) => {
                bcmp::Compare(&x_k, &y_k)
            },
            (&KeyRef::Boxed(ref x_k), &KeyRef::Prefixed(ref y_p, ref y_k)) => {
                Self::compare_x_py(&x_k, y_p, y_k)
            },
            (&KeyRef::Boxed(ref x_k), &KeyRef::Array(ref y_k)) => {
                bcmp::Compare(&x_k, &y_k)
            },
            (&KeyRef::Prefixed(ref x_p, ref x_k), &KeyRef::Boxed(ref y_k)) => {
                Self::compare_px_y(x_p, x_k, &y_k)
            },
            (&KeyRef::Array(ref x_k), &KeyRef::Boxed(ref y_k)) => {
                bcmp::Compare(&x_k, &y_k)
            },
            (&KeyRef::Prefixed(ref x_p, ref x_k), &KeyRef::Prefixed(ref y_p, ref y_k)) => {
                Self::compare_px_py(x_p, x_k, y_p, y_k)
            },
            (&KeyRef::Prefixed(ref x_p, ref x_k), &KeyRef::Array(ref y_k)) => {
                Self::compare_px_y(x_p, x_k, y_k)
            },
            (&KeyRef::Array(ref x_k), &KeyRef::Prefixed(ref y_p, ref y_k)) => {
                Self::compare_x_py(x_k, y_p, y_k)
            },
            (&KeyRef::Array(ref x_k), &KeyRef::Array(ref y_k)) => {
                bcmp::Compare(&x_k, &y_k)
            },
        }
    }
}


/// Like a ValueRef, but cannot represent a tombstone.  Available
/// only from a LivingCursor.
pub enum LiveValueRef<'a> {
    Array(&'a [u8]),
    // TODO return the information about the overflow, but don't
    // open the stream yet.  for the merge case, we want to allow
    // for the overflow to be moved without copying it.
    Overflowed(usize, Box<Read>),
}

impl<'a> LiveValueRef<'a> {
    pub fn len(&self) -> usize {
        match *self {
            LiveValueRef::Array(a) => a.len(),
            LiveValueRef::Overflowed(len, _) => len,
        }
    }

    pub fn into_blob(self) -> Blob {
        match self {
            LiveValueRef::Array(a) => {
                let mut k = Vec::with_capacity(a.len());
                k.push_all(a);
                Blob::Array(k.into_boxed_slice())
            },
            LiveValueRef::Overflowed(_, r) => Blob::Stream(r),
        }
    }

    // dangerous function if len() is big
    pub fn into_boxed_slice(self) -> Result<Box<[u8]>> {
        match self {
            LiveValueRef::Array(a) => {
                let mut v = Vec::with_capacity(a.len());
                v.push_all(a);
                Ok(v.into_boxed_slice())
            },
            LiveValueRef::Overflowed(len, mut strm) => {
                let mut a = Vec::with_capacity(len);
                try!(strm.read_to_end(&mut a));
                Ok(a.into_boxed_slice())
            },
        }
    }

    /// A LiveValueRef is conceptually just a bunch of bytes, but it can be represented in
    /// two different ways, depending on whether the value in the page was overflowed
    /// or not.  This function accepts a func which is to be applied to the value.
    /// If the value was overflowed, it will be read into memory.  Otherwise, the func
    /// gets called without alloc or copy.
    pub fn map<T, F: Fn(&[u8]) -> Result<T>>(self, func: F) -> Result<T> {
        match self {
            LiveValueRef::Array(a) => {
                let t = try!(func(a));
                Ok(t)
            },
            LiveValueRef::Overflowed(len, mut strm) => {
                let mut a = Vec::with_capacity(len);
                try!(strm.read_to_end(&mut a));
                assert!(len == a.len());
                let t = try!(func(&a));
                Ok(t)
            },
        }
    }
}

impl<'a> std::fmt::Debug for LiveValueRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match *self {
            LiveValueRef::Array(a) => write!(f, "Array, len={:?}", a),
            LiveValueRef::Overflowed(klen,_) => write!(f, "Overflowed, len={}", klen),
        }
    }
}

pub enum ValueRef<'a> {
    Array(&'a [u8]),
    // TODO return the information about the overflow, but don't
    // open the stream yet.  for the merge case, we want to allow
    // for the overflow to be moved without copying it.
    Overflowed(usize, Box<Read>),
    Tombstone,
}

impl<'a> ValueRef<'a> {
    pub fn len(&self) -> Option<usize> {
        match *self {
            ValueRef::Array(a) => Some(a.len()),
            ValueRef::Overflowed(len, _) => Some(len),
            ValueRef::Tombstone => None,
        }
    }

    pub fn into_blob(self) -> Blob {
        match self {
            ValueRef::Array(a) => {
                let mut k = Vec::with_capacity(a.len());
                k.push_all(a);
                Blob::Array(k.into_boxed_slice())
            },
            ValueRef::Overflowed(_, r) => Blob::Stream(r),
            ValueRef::Tombstone => Blob::Tombstone,
        }
    }

}

impl<'a> std::fmt::Debug for ValueRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match *self {
            ValueRef::Array(a) => write!(f, "Array, len={:?}", a),
            ValueRef::Overflowed(klen,_) => write!(f, "Overflowed, len={}", klen),
            ValueRef::Tombstone => write!(f, "Tombstone"),
        }
    }
}

// TODO consider putting this into a module so we can keep firstPage and lastPage private and
// expose methods to modify them so that we can assert when they become invalid.

#[derive(Hash,PartialEq,Eq,Copy,Clone,Debug)]
pub struct PageBlock {
    firstPage: PageNum,
    lastPage: PageNum,
}

impl PageBlock {
    pub fn new(first: PageNum, last: PageNum) -> PageBlock {
        assert!(first <= last);
        PageBlock { firstPage: first, lastPage: last }
    }

    pub fn count_pages(&self) -> PageNum {
        self.lastPage - self.firstPage + 1
    }

    fn contains_page(&self, pgnum: PageNum) -> bool {
        (pgnum >= self.firstPage) && (pgnum <= self.lastPage)
    }
}

pub type SegmentNum = u64;

#[derive(PartialEq,Copy,Clone,Debug)]
pub enum SeekOp {
    SEEK_EQ = 0,
    SEEK_LE = 1,
    SEEK_GE = 2,
}

struct CursorIterator {
    csr: Box<ICursor>,
}

impl CursorIterator {
    fn new(it: Box<ICursor>) -> CursorIterator {
        CursorIterator { csr: it }
    }
}

impl Iterator for CursorIterator {
    type Item = Result<kvp>;
    fn next(&mut self) -> Option<Result<kvp>> {
        if self.csr.IsValid() {
            let k = {
                let k = self.csr.KeyRef();
                if k.is_err() {
                    return Some(Err(k.err().unwrap()));
                }
                let k = k.unwrap().into_boxed_slice();
                k
            };
            let v = {
                let v = self.csr.ValueRef();
                if v.is_err() {
                    return Some(Err(v.err().unwrap()));
                }
                let v = v.unwrap().into_blob();
                v
            };
            let r = self.csr.Next();
            if r.is_err() {
                return Some(Err(r.err().unwrap()));
            }
            Some(Ok(kvp{Key:k, Value:v}))
        } else {
            return None;
        }
    }
}

#[derive(Copy,Clone,Debug,PartialEq)]
pub enum SeekResult {
    Invalid,
    Unequal,
    Equal,
}

impl SeekResult {
    fn verify(&self, k: &KeyRef, sop: SeekOp, csr: &ICursor) -> Result<()> {
        match self {
            &SeekResult::Invalid => {
                assert!(!csr.IsValid());
            },
            _ => {
                let q = try!(csr.KeyRef());
                let cmp = KeyRef::cmp(&q, k);
                match sop {
                    SeekOp::SEEK_LE => {
                        assert!(cmp == Ordering::Less || cmp == Ordering::Equal);
                    },
                    SeekOp::SEEK_GE => {
                        assert!(cmp == Ordering::Greater || cmp == Ordering::Equal);
                    },
                    SeekOp::SEEK_EQ => {
                        assert!(cmp == Ordering::Equal || cmp == Ordering::Equal);
                    },
                }
            }
        }
        Ok(())
    }

    fn from_cursor(csr: &ICursor, k: &KeyRef) -> Result<SeekResult> {
        if csr.IsValid() {
            let kc = try!(csr.KeyRef());
            if Ordering::Equal == KeyRef::cmp(&kc, k) {
                Ok(SeekResult::Equal)
            } else {
                Ok(SeekResult::Unequal)
            }
        } else {
            Ok(SeekResult::Invalid)
        }
    }

    fn is_valid(self) -> bool {
        match self {
            SeekResult::Invalid => false,
            SeekResult::Unequal => true,
            SeekResult::Equal => true,
        }
    }

    fn is_valid_and_equal(self) -> bool {
        match self {
            SeekResult::Invalid => false,
            SeekResult::Unequal => false,
            SeekResult::Equal => true,
        }
    }
}

pub trait ICursor {
    fn SeekRef(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult>;
    fn First(&mut self) -> Result<()>;
    fn Last(&mut self) -> Result<()>;
    fn Next(&mut self) -> Result<()>;
    fn Prev(&mut self) -> Result<()>;

    fn IsValid(&self) -> bool;

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>>;
    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>>;

    // TODO maybe rm ValueLength.  but LivingCursor uses it as a fast
    // way to detect whether a value is a tombstone or not.
    fn ValueLength(&self) -> Result<Option<usize>>; // tombstone is None
    // TODO ValueLength type should be u64

}

//#[derive(Copy,Clone)]
pub struct DbSettings {
    pub DefaultPageSize: usize,
    pub PagesPerBlock: PageNum,
}

pub const DEFAULT_SETTINGS: DbSettings = 
    DbSettings {
        DefaultPageSize: 4096,
        PagesPerBlock: 256,
    };

#[derive(Debug,Clone)]
pub struct SegmentLocation {
    root_page: PageNum,
    blocks: BlockList,
}

impl SegmentLocation {
    pub fn new(root_page: PageNum, blocks: BlockList) -> Self {
        assert!(blocks.contains_page(root_page));
        SegmentLocation {
            root_page: root_page,
            blocks: blocks,
        }
    }

    pub fn count_pages(&self) -> PageCount {
        self.blocks.count_pages()
    }

    fn contains_page(&self, pgnum: PageNum) -> bool {
        self.blocks.contains_page(pgnum)
    }

}

// TODO this will probably go away again
#[derive(Debug,Clone)]
pub struct SegmentInfo {
    level: u32,
    location: SegmentLocation,
}

impl SegmentInfo {
    pub fn count_pages(&self) -> PageCount {
        self.location.count_pages()
    }
}

// TODO why is this pub?
// TODO why is this even a module?
pub mod utils {
    use std::io::Seek;
    use std::io::SeekFrom;
    use super::PageNum;
    use super::Error;
    use super::Result;

    pub fn SeekPage(strm: &mut Seek, pgsz: usize, pageNumber: PageNum) -> Result<u64> {
        if 0 == pageNumber { 
            // TODO should this panic?
            return Err(Error::InvalidPageNumber);
        }
        let pos = ((pageNumber as u64) - 1) * (pgsz as u64);
        let v = try!(strm.seek(SeekFrom::Start(pos)));
        Ok(v)
    }

}

mod bcmp {
    use std::cmp::Ordering;
    use std::cmp::min;

    // this fn is actually kinda handy to make sure that we are comparing
    // what we are supposed to be comparing.  if we just use cmp, we
    // can end up comparing two things that happen to match each other's
    // type but which do not match &[u8].  this function makes the type
    // checking more explicit.
    #[inline(always)]
    pub fn Compare(x: &[u8], y: &[u8]) -> Ordering {
        x.cmp(y)
    }

    pub fn PrefixMatch(x: &[u8], y: &[u8], max: usize) -> usize {
        let len = min(x.len(), y.len());
        let lim = min(len, max);
        let mut i = 0;
        while i < lim && x[i] == y[i] {
            i = i + 1;
        }
        i
    }

    #[cfg(remove_me)]
    fn StartsWith(x: &[u8], y: &[u8], max: usize) -> bool {
        if x.len() < y.len() {
            false
        } else {
            let len = y.len();
            let mut i = 0;
            while i<len && x[i]==y[i] {
                i = i + 1;
            }
            i==len
        }
    }
}

struct PageBuilder {
    cur : usize,
    buf : Box<[u8]>,
}

// TODO bundling cur with the buf almost seems sad, because there are
// cases where we want buf to be mutable but not cur.  :-)

impl PageBuilder {
    fn new(pgsz : usize) -> PageBuilder { 
        let ba = vec![0;pgsz as usize].into_boxed_slice();
        PageBuilder { cur: 0, buf:ba } 
    }

    fn buf(&self) -> &[u8] {
        &self.buf
    }

    fn Reset(&mut self) {
        self.cur = 0;
    }

    fn sofar(&self) -> usize {
        self.cur
    }

    fn Write(&self, strm: &mut Write) -> io::Result<()> {
        strm.write_all(&*self.buf)
    }

    fn Available(&self) -> usize {
        self.buf.len() - self.cur
    }

    fn SetPageFlag(&mut self, x: u8) {
        self.buf[1] = self.buf[1] | (x);
    }

    fn PutByte(&mut self, x: u8) {
        self.buf[self.cur] = x;
        self.cur = self.cur + 1;
    }

    fn PutStream2(&mut self, s: &mut Read, len: usize) -> io::Result<usize> {
        let n = try!(misc::io::read_fully(s, &mut self.buf[self.cur .. self.cur + len]));
        self.cur = self.cur + n;
        let res : io::Result<usize> = Ok(n);
        res
    }

    fn PutArray(&mut self, ba: &[u8]) {
        self.buf[self.cur .. self.cur + ba.len()].clone_from_slice(ba);
        self.cur = self.cur + ba.len();
    }

    fn PutInt32(&mut self, ov: u32) {
        let at = self.cur;
        // TODO just self.buf?  instead of making 4-byte slice.
        misc::bytes::copy_into(&endian::u32_to_bytes_be(ov), &mut self.buf[at .. at + SIZE_32]);
        self.cur = self.cur + SIZE_32;
    }

    fn SetLastInt32(&mut self, page: u32) {
        let len = self.buf.len();
        let at = len - 1 * SIZE_32;
        if self.cur > at { panic!("SetLastInt32 is squashing data"); }
        // TODO just self.buf?  instead of making 4-byte slice.
        misc::bytes::copy_into(&endian::u32_to_bytes_be(page), &mut self.buf[at .. at + SIZE_32]);
    }

    fn PutInt16(&mut self, ov: u16) {
        let at = self.cur;
        // TODO just self.buf?  instead of making 2-byte slice.
        misc::bytes::copy_into(&endian::u16_to_bytes_be(ov), &mut self.buf[at .. at + SIZE_16]);
        self.cur = self.cur + SIZE_16;
    }

    fn PutVarint(&mut self, ov: u64) {
        varint::write(&mut *self.buf, &mut self.cur, ov);
    }

}

#[derive(PartialEq,Copy,Clone)]
enum Direction {
    // TODO why do we need to assign these specific values?
    Forward = 0,
    Backward = 1,
    Wandering = 2,
}

struct MultiCursor { 
    subcursors: Box<[Lend<PageCursor>]>, 
    sorted: Box<[(usize, Option<Ordering>)]>,
    cur: Option<usize>, 
    dir: Direction,
}

impl MultiCursor {
    fn sort(&mut self, want_max: bool) -> Result<()> {
        if self.subcursors.is_empty() {
            return Ok(())
        }

        // TODO this memory allocation is expensive.

        // get a KeyRef for all the cursors
        let mut ka = Vec::with_capacity(self.subcursors.len());
        for c in self.subcursors.iter() {
            if c.IsValid() {
                ka.push(Some(try!(c.KeyRef())));
            } else {
                ka.push(None);
            }
        }

        // TODO consider converting ka to a boxed slice here?

        // init the orderings to None.
        // the invalid cursors will stay that way.
        for i in 0 .. self.sorted.len() {
            self.sorted[i].1 = None;
        }

        for i in 1 .. self.sorted.len() {
            let mut j = i;
            while j > 0 {
                let nj = self.sorted[j].0;
                let nprev = self.sorted[j - 1].0;
                match (&ka[nj], &ka[nprev]) {
                    (&Some(ref kj), &Some(ref kprev)) => {
                        let c = {
                            if want_max {
                                KeyRef::cmp(kprev, kj)
                            } else {
                                KeyRef::cmp(kj, kprev)
                            }
                        };
                        match c {
                            Ordering::Greater => {
                                self.sorted[j].1 = Some(Ordering::Greater);
                                break;
                            },
                            Ordering::Equal => {
                                match nj.cmp(&nprev) {
                                    Ordering::Equal => {
                                        unreachable!();
                                    },
                                    Ordering::Greater => {
                                        self.sorted[j].1 = Some(Ordering::Equal);
                                        break;
                                    },
                                    Ordering::Less => {
                                        self.sorted[j - 1].1 = Some(Ordering::Equal);
                                        // keep going
                                    },
                                }
                            },
                            Ordering::Less => {
                                // keep going
                                self.sorted[j - 1].1 = Some(Ordering::Greater);
                            },
                        }
                    },
                    (&Some(_), &None) => {
                        // keep going
                    },
                    (&None, &Some(_)) => {
                        break;
                    },
                    (&None, &None) => {
                        match nj.cmp(&nprev) {
                            Ordering::Equal => {
                                unreachable!();
                            },
                            Ordering::Greater => {
                                break;
                            },
                            Ordering::Less => {
                                // keep going
                            },
                        }
                    }
                };
                self.sorted.swap(j, j - 1);
                j = j - 1;
            }
        }

        // fix the first one
        if self.sorted.len() > 0 {
            let n = self.sorted[0].0;
            if ka[n].is_some() {
                self.sorted[0].1 = Some(Ordering::Equal);
            }
        }

        /*
        println!("{:?} : {}", self.sorted, if want_max { "backward" } else {"forward"} );
        for i in 0 .. self.sorted.len() {
            let (n, ord) = self.sorted[i];
            println!("    {:?}", ka[n]);
        }
        */
        Ok(())
    }

    fn sorted_first(&self) -> Option<usize> {
        let n = self.sorted[0].0;
        if self.sorted[0].1.is_some() {
            Some(n)
        } else {
            None
        }
    }

    fn findMin(&mut self) -> Result<Option<usize>> {
        self.dir = Direction::Forward;
        if self.subcursors.is_empty() {
            Ok(None)
        } else {
            try!(self.sort(false));
            Ok(self.sorted_first())
        }
    }

    fn findMax(&mut self) -> Result<Option<usize>> {
        self.dir = Direction::Backward; 
        if self.subcursors.is_empty() {
            Ok(None)
        } else {
            try!(self.sort(true));
            Ok(self.sorted_first())
        }
    }

    fn Create(subs: Vec<Lend<PageCursor>>) -> MultiCursor {
        let s = subs.into_boxed_slice();
        let mut sorted = Vec::with_capacity(s.len());
        for i in 0 .. s.len() {
            sorted.push((i, None));
        }
        MultiCursor { 
            subcursors: s, 
            sorted: sorted.into_boxed_slice(), 
            cur: None, 
            dir: Direction::Wandering,
        }
    }

    fn seek(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        self.cur = None;
        self.dir = Direction::Wandering;
        for j in 0 .. self.subcursors.len() {
            let sr = try!(self.subcursors[j].SeekRef(k, sop));
            if sr.is_valid_and_equal() { 
                self.cur = Some(j);
                return Ok(sr);
            }
        }
        match sop {
            SeekOp::SEEK_GE => {
                self.cur = try!(self.findMin());
                match self.cur {
                    Some(i) => {
                        SeekResult::from_cursor(&*self.subcursors[i], k)
                    },
                    None => {
                        Ok(SeekResult::Invalid)
                    },
                }
            },
            SeekOp::SEEK_LE => {
                self.cur = try!(self.findMax());
                match self.cur {
                    Some(i) => {
                        SeekResult::from_cursor(&*self.subcursors[i], k)
                    },
                    None => {
                        Ok(SeekResult::Invalid)
                    },
                }
            },
            SeekOp::SEEK_EQ => {
                Ok(SeekResult::Invalid)
            },
        }
    }

}

impl ICursor for MultiCursor {
    fn IsValid(&self) -> bool {
        match self.cur {
            Some(i) => self.subcursors[i].IsValid(),
            None => false
        }
    }

    fn First(&mut self) -> Result<()> {
        for i in 0 .. self.subcursors.len() {
            try!(self.subcursors[i].First());
        }
        self.cur = try!(self.findMin());
        Ok(())
    }

    fn Last(&mut self) -> Result<()> {
        for i in 0 .. self.subcursors.len() {
            try!(self.subcursors[i].Last());
        }
        self.cur = try!(self.findMax());
        Ok(())
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(icur) => {
                self.subcursors[icur].KeyRef()
            },
        }
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(icur) => {
                self.subcursors[icur].ValueRef()
            },
        }
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(icur) => {
                self.subcursors[icur].ValueLength()
            },
        }
    }

    fn Next(&mut self) -> Result<()> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(icur) => {
                // we need to fix every cursor to point to its min
                // value > icur.

                // if perf didn't matter, this would be simple.
                // call Next on icur.  and call Seek(GE) (and maybe Next)
                // on every other cursor.

                // but there are several cases where we can do a lot
                // less work than a Seek.  And we have the information
                // to identify those cases.  So, this function is
                // pretty complicated, but it's fast.

                // --------

                // the current cursor (icur) is easy.  it just needs Next().
                // we'll do it last, so we can use it for comparisons.
                // for now we deal with all the others.

                // the current direction of the multicursor tells us
                // something about the state of all the others.

                if self.dir == Direction::Forward {
                    // this is the happy case.  each cursor is at most
                    // one step away.

                    // direction is Forward, so we know that every valid cursor
                    // is pointing at a key which is either == to icur, or
                    // it is already the min key > icur.

                    assert!(icur == self.sorted[0].0);
                    // immediately after that, there may (or may not be) some
                    // entries which were Ordering:Equal to cur.  call Next on
                    // each of these.

                    for i in 1 .. self.sorted.len() {
                        //println!("sorted[{}] : {:?}", i, self.sorted[i]);
                        let (n, c) = self.sorted[i];
                        match c {
                            None => {
                                break;
                            },
                            Some(c) => {
                                if c == Ordering::Equal {
                                    try!(self.subcursors[n].Next());
                                } else {
                                    break;
                                }
                            },
                        }
                    }

                } else {
                    // TODO consider simplifying all the stuff below.
                    // all this complexity may not be worth it.

                    fn half(dir: Direction, ki: &KeyRef, subs: &mut [Lend<PageCursor>]) -> Result<()> {
                        match dir {
                            Direction::Forward => {
                                unreachable!();
                            },
                            Direction::Backward => {
                                // this case isn't too bad.  each cursor is either
                                // one step away or two.
                                
                                // every other cursor is either == icur or it is the
                                // max value < icur.

                                for csr in subs {
                                    if csr.IsValid() {
                                        let cmp = {
                                            let k = try!(csr.KeyRef());
                                            let cmp = KeyRef::cmp(&k, ki);
                                            cmp
                                        };
                                        match cmp {
                                            Ordering::Less => {
                                                try!(csr.Next());
                                                // we moved one step.  let's see if we need to move one more.
                                                if csr.IsValid() {
                                                    let cmp = {
                                                        let k = try!(csr.KeyRef());
                                                        let cmp = KeyRef::cmp(&k, ki);
                                                        cmp
                                                    };
                                                    match cmp {
                                                        Ordering::Less => {
                                                            // should never happen.  we should not have
                                                            // been more than one step away from icur.
                                                            unreachable!();
                                                        },
                                                        Ordering::Greater => {
                                                            // done
                                                        },
                                                        Ordering::Equal => {
                                                            // and one more step
                                                            try!(csr.Next());
                                                        },
                                                    }
                                                }
                                            },
                                            Ordering::Greater => {
                                                // should never happen, because Backward
                                                unreachable!();
                                            },
                                            Ordering::Equal => {
                                                // one step away
                                                try!(csr.Next());
                                            },
                                        }
                                    } else {
                                        let sr = try!(csr.SeekRef(&ki, SeekOp::SEEK_GE));
                                        if sr.is_valid_and_equal() {
                                            try!(csr.Next());
                                        }
                                    }
                                }

                                Ok(())
                            },
                            Direction::Wandering => {
                                // we have no idea where all the other cursors are.
                                // so we have to do a seek on each one.

                                for j in 0 .. subs.len() {
                                    let csr = &mut subs[j];
                                    let sr = try!(csr.SeekRef(&ki, SeekOp::SEEK_GE));
                                    if sr.is_valid_and_equal() {
                                        try!(csr.Next());
                                    }
                                }
                                Ok(())
                            },
                        }
                    }

                    {
                        let (before, middle, after) = split3(&mut *self.subcursors, icur);
                        let icsr = &middle[0];
                        let ki = try!(icsr.KeyRef());
                        try!(half(self.dir, &ki, before));
                        try!(half(self.dir, &ki, after));
                    }
                }

                // now the current cursor
                try!(self.subcursors[icur].Next());

                // now re-sort
                self.cur = try!(self.findMin());
                Ok(())
            },
        }
    }

    // TODO fix Prev like Next
    fn Prev(&mut self) -> Result<()> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(icur) => {
                let k = {
                    let k = try!(self.subcursors[icur].KeyRef());
                    let k = k.into_boxed_slice();
                    let k = KeyRef::from_boxed_slice(k);
                    k
                };
                for j in 0 .. self.subcursors.len() {
                    let csr = &mut self.subcursors[j];
                    if (self.dir != Direction::Backward) && (icur != j) { 
                        try!(csr.SeekRef(&k, SeekOp::SEEK_LE));
                    }
                    if csr.IsValid() {
                        let eq = {
                            let kc = try!(csr.KeyRef());
                            Ordering::Equal == KeyRef::cmp(&kc, &k)
                        };
                        if eq {
                            try!(csr.Prev());
                        }
                    }
                }
                self.cur = try!(self.findMax());
                Ok(())
            },
        }
    }

    fn SeekRef(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        //println!("MC SeekRef  k={:?}  sop={:?}", k, sop);
        let sr = try!(self.seek(k, sop));
        if cfg!(expensive_check) 
        {
            try!(sr.verify(k, sop, self));
        }
        Ok(sr)
    }

}

pub struct LivingCursor { 
    chain: MultiCursor
}

impl LivingCursor {
    pub fn LiveValueRef<'a>(&'a self) -> Result<LiveValueRef<'a>> {
        match try!(self.chain.ValueRef()) {
            ValueRef::Array(a) => Ok(LiveValueRef::Array(a)),
            ValueRef::Overflowed(len, r) => Ok(LiveValueRef::Overflowed(len, r)),
            ValueRef::Tombstone => Err(Error::Misc(String::from("LiveValueRef tombstone TODO unreachable"))),
        }
    }

    fn skipTombstonesForward(&mut self) -> Result<()> {
        while self.chain.IsValid() && try!(self.chain.ValueLength()).is_none() {
            try!(self.chain.Next());
        }
        Ok(())
    }

    fn skipTombstonesBackward(&mut self) -> Result<()> {
        while self.chain.IsValid() && try!(self.chain.ValueLength()).is_none() {
            try!(self.chain.Prev());
        }
        Ok(())
    }

    fn Create(ch : MultiCursor) -> LivingCursor {
        LivingCursor { chain : ch }
    }
}

impl ICursor for LivingCursor {
    fn First(&mut self) -> Result<()> {
        try!(self.chain.First());
        try!(self.skipTombstonesForward());
        Ok(())
    }

    fn Last(&mut self) -> Result<()> {
        try!(self.chain.Last());
        try!(self.skipTombstonesBackward());
        Ok(())
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        self.chain.KeyRef()
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        self.chain.ValueRef()
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        self.chain.ValueLength()
    }

    fn IsValid(&self) -> bool {
        self.chain.IsValid() 
            && {
                let r = self.chain.ValueLength();
                if r.is_ok() {
                    r.unwrap().is_some()
                } else {
                    false
                }
            }
    }

    fn Next(&mut self) -> Result<()> {
        try!(self.chain.Next());
        try!(self.skipTombstonesForward());
        Ok(())
    }

    fn Prev(&mut self) -> Result<()> {
        try!(self.chain.Prev());
        try!(self.skipTombstonesBackward());
        Ok(())
    }

    fn SeekRef(&mut self, k: &KeyRef, sop:SeekOp) -> Result<SeekResult> {
        //println!("Living SeekRef  k={:?}  sop={:?}", k, sop);
        let sr = try!(self.chain.SeekRef(k, sop));
        if cfg!(expensive_check) 
        {
            try!(sr.verify(k, sop, self));
        }
        match sop {
            SeekOp::SEEK_GE => {
                if sr.is_valid() && self.chain.ValueLength().unwrap().is_none() {
                    try!(self.skipTombstonesForward());
                    SeekResult::from_cursor(&self.chain, k)
                } else {
                    Ok(sr)
                }
            },
            SeekOp::SEEK_LE => {
                if sr.is_valid() && self.chain.ValueLength().unwrap().is_none() {
                    try!(self.skipTombstonesBackward());
                    SeekResult::from_cursor(&self.chain, k)
                } else {
                    Ok(sr)
                }
            },
            SeekOp::SEEK_EQ => Ok(sr),
        }
    }

}

pub struct FilterTombstonesCursor { 
    chain: MultiCursor,
    behind: Vec<Lend<PageCursor>>,
}

impl FilterTombstonesCursor {
    fn can_skip(&mut self) -> Result<bool> {
        if self.chain.IsValid() && try!(self.chain.ValueLength()).is_none() {
            let k = try!(self.chain.KeyRef());
            // TODO would it be faster to just keep behind moving Next() along with chain?
            // then we could optimize cases where we already know that the key is not present
            // in behind because, for example, we hit the max key in behind already.

            for mut cursor in self.behind.iter_mut() {
                if SeekResult::Equal == try!(cursor.SeekRef(&k, SeekOp::SEEK_EQ)) {
                    // TODO if the value was found but it is another tombstone, then it is actually
                    // not found
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn skipTombstonesForward(&mut self) -> Result<()> {
        while try!(self.can_skip()) {
            try!(self.chain.Next());
        }
        Ok(())
    }

    fn skipTombstonesBackward(&mut self) -> Result<()> {
        while try!(self.can_skip()) {
            try!(self.chain.Prev());
        }
        Ok(())
    }

    fn new(ch: MultiCursor, behind: Vec<Lend<PageCursor>>) -> FilterTombstonesCursor {
        FilterTombstonesCursor { 
            chain: ch,
            behind: behind,
        }
    }
}

impl ICursor for FilterTombstonesCursor {
    fn First(&mut self) -> Result<()> {
        try!(self.chain.First());
        try!(self.skipTombstonesForward());
        Ok(())
    }

    fn Last(&mut self) -> Result<()> {
        try!(self.chain.Last());
        try!(self.skipTombstonesBackward());
        Ok(())
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        self.chain.KeyRef()
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        self.chain.ValueRef()
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        self.chain.ValueLength()
    }

    fn IsValid(&self) -> bool {
        self.chain.IsValid() 
    }

    fn Next(&mut self) -> Result<()> {
        try!(self.chain.Next());
        try!(self.skipTombstonesForward());
        Ok(())
    }

    fn Prev(&mut self) -> Result<()> {
        try!(self.chain.Prev());
        try!(self.skipTombstonesBackward());
        Ok(())
    }

    fn SeekRef(&mut self, k: &KeyRef, sop:SeekOp) -> Result<SeekResult> {
        let sr = try!(self.chain.SeekRef(k, sop));
        match sop {
            SeekOp::SEEK_GE => {
                if sr.is_valid() && self.chain.ValueLength().unwrap().is_none() {
                    try!(self.skipTombstonesForward());
                    SeekResult::from_cursor(&self.chain, k)
                } else {
                    Ok(sr)
                }
            },
            SeekOp::SEEK_LE => {
                if sr.is_valid() && self.chain.ValueLength().unwrap().is_none() {
                    try!(self.skipTombstonesBackward());
                    SeekResult::from_cursor(&self.chain, k)
                } else {
                    Ok(sr)
                }
            },
            SeekOp::SEEK_EQ => Ok(sr),
        }
    }

}

#[derive(PartialEq,Copy,Clone,Debug)]
pub enum OpLt {
    LT,
    LTE,
}

#[derive(PartialEq,Copy,Clone,Debug)]
pub enum OpGt {
    GT,
    GTE,
}

#[derive(Debug)]
pub struct Min {
    k: Box<[u8]>,
    cmp: OpGt,
}

impl Min {
    pub fn new(k: Box<[u8]>, cmp: OpGt) -> Self {
        Min {
            k: k,
            cmp: cmp,
        }
    }

    fn is_in_bounds(&self, k: &KeyRef) -> bool {
        let c = k.compare_with(&self.k);
        match (self.cmp, c) {
            (OpGt::GT, Ordering::Greater) => true,
            (OpGt::GT, Ordering::Less) => false,
            (OpGt::GT, Ordering::Equal) => false,
            (OpGt::GTE, Ordering::Greater) => true,
            (OpGt::GTE, Ordering::Less) => false,
            (OpGt::GTE, Ordering::Equal) => true,
        }
    }
}

#[derive(Debug)]
pub struct Max {
    k: Box<[u8]>,
    cmp: OpLt,
}

impl Max {
    pub fn new(k: Box<[u8]>, cmp: OpLt) -> Self {
        Max {
            k: k,
            cmp: cmp,
        }
    }

    fn is_in_bounds(&self, k: &KeyRef) -> bool {
        let c = k.compare_with(&self.k);
        match (self.cmp, c) {
            (OpLt::LT, Ordering::Greater) => false,
            (OpLt::LT, Ordering::Less) => true,
            (OpLt::LT, Ordering::Equal) => false,
            (OpLt::LTE, Ordering::Greater) => false,
            (OpLt::LTE, Ordering::Less) => true,
            (OpLt::LTE, Ordering::Equal) => true,
        }
    }
}

pub struct RangeCursor { 
    chain: LivingCursor,
    min: Min,
    max: Max,
}

impl RangeCursor {
    pub fn new(ch: LivingCursor, min: Min, max: Max) -> RangeCursor {
        //println!("RangeCursor min: {:?}", min);
        //println!("RangeCursor max: {:?}", max);
        RangeCursor { 
            chain : ch,
            min: min,
            max: max,
        }
    }

    pub fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        if self.IsValid() {
            self.chain.KeyRef()
        } else {
            Err(Error::CursorNotValid)
        }
    }

    pub fn LiveValueRef<'a>(&'a self) -> Result<LiveValueRef<'a>> {
        if self.IsValid() {
            self.chain.LiveValueRef()
        } else {
            Err(Error::CursorNotValid)
        }
    }

    pub fn IsValid(&self) -> bool {
        self.chain.IsValid() 
            && {
                let k = self.chain.KeyRef().unwrap();
                //println!("bounds checking: {:?}", k);
                self.min.is_in_bounds(&k) && self.max.is_in_bounds(&k)
            }
    }

    pub fn First(&mut self) -> Result<()> {
        let sr = try!(self.chain.SeekRef(&KeyRef::for_slice(&self.min.k), SeekOp::SEEK_GE));
        match (sr, self.min.cmp) {
            (SeekResult::Equal, OpGt::GT) => {
                try!(self.chain.Next());
            },
            _ => {
            },
        }
        Ok(())
    }

    pub fn Next(&mut self) -> Result<()> {
        if self.IsValid() {
            self.chain.Next()
        } else {
            Err(Error::CursorNotValid)
        }
    }

    // TODO this one could support Last/Prev I suppose
}

pub struct PrefixCursor<'c> { 
    chain : &'c mut LivingCursor,
    prefix: Box<[u8]>,
}

impl<'c> PrefixCursor<'c> {
    pub fn new(ch: &'c mut LivingCursor, prefix: Box<[u8]>) -> PrefixCursor<'c> {
        //println!("PrefixCursor new: {:?}", prefix);
        PrefixCursor { 
            chain : ch,
            prefix: prefix,
        }
    }

    // TODO lifetimes below should be 'c ?
    pub fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        if self.IsValid() {
            let k = try!(self.chain.KeyRef());
            //println!("PrefixCursor yielding: {:?}", k);
            //assert!(k.starts_with(&self.prefix));
            Ok(k)
        } else {
            Err(Error::CursorNotValid)
        }
    }

    // TODO lifetimes below should be 'c ?
    pub fn LiveValueRef<'a>(&'a self) -> Result<LiveValueRef<'a>> {
        if self.IsValid() {
            self.chain.LiveValueRef()
        } else {
            Err(Error::CursorNotValid)
        }
    }

    pub fn IsValid(&self) -> bool {
        self.chain.IsValid() 
            && 
            {
                let k = self.chain.KeyRef().unwrap();
                //println!("PrefixCursor chain is valid, its k={:?}", k);
                k.starts_with(&self.prefix)
            }
    }

    pub fn First(&mut self) -> Result<SeekResult> {
        let sr = try!(self.chain.SeekRef(&KeyRef::for_slice(&self.prefix), SeekOp::SEEK_GE));
        Ok(sr)
    }

    pub fn Next(&mut self) -> Result<()> {
        if self.IsValid() {
            try!(self.chain.Next());
            Ok(())
        } else {
            Err(Error::CursorNotValid)
        }
    }

}

#[derive(Hash,PartialEq,Eq,Copy,Clone,Debug)]
#[repr(u8)]
pub enum PageType {
    LEAF_NODE,
    PARENT_NODE,
    OVERFLOW_NODE,
}

impl PageType {

    #[inline(always)]
    fn to_u8(self) -> u8 {
        match self {
            PageType::LEAF_NODE => 1,
            PageType::PARENT_NODE => 2,
            PageType::OVERFLOW_NODE => 3,
        }
    }

    #[inline(always)]
    fn from_u8(v: u8) -> Result<PageType> {
        match v {
            1 => Ok(PageType::LEAF_NODE),
            2 => Ok(PageType::PARENT_NODE),
            3 => Ok(PageType::OVERFLOW_NODE),
            _ => panic!(), // TODO Err(Error::InvalidPageType(v)),
        }
    }
}

mod ValueFlag {
    pub const FLAG_OVERFLOW: u8 = 1;
    pub const FLAG_TOMBSTONE: u8 = 2;
}

mod PageFlag {
    pub const FLAG_BOUNDARY_NODE: u8 = 2;
}

#[derive(Debug)]
// this struct is used to remember pages we have written.
struct pgitem {
    page: PageNum,
    blocks: BlockList,
    // blocks does NOT contain page

    first_key: KeyWithLocation,
    last_key: Option<KeyWithLocation>,
}

impl pgitem {
    fn need(&self, prefix_len: usize) -> usize {
        let needed = 
            varint::space_needed_for(self.page as u64) 
            //if cfg!(child_block_list) 
            + self.blocks.encoded_len()
            + self.first_key.need(prefix_len)
            + 
            match self.last_key {
                Some(ref last_key) => {
                    last_key.need(prefix_len)
                },
                None => {
                    varint::space_needed_for(0)
                },
            };
        needed
    }
}

struct ParentState {
    prefixLen: usize,
    sofar: usize,
    items: Vec<pgitem>,
    nextGeneration: Vec<pgitem>,
}

// this enum keeps track of what happened to a key as we
// processed it.  either we determined that it will fit
// inline or we wrote it as an overflow.
#[derive(Debug)]
enum KeyLocation {
    Inline,
    Overflowed(BlockList),
}

impl ValueLocation {
    fn need(&self) -> usize {
        match self {
            &ValueLocation::Tombstone => {
                1
            },
            &ValueLocation::Buffer(ref vbuf) => {
                let vlen = vbuf.len();
                1 + varint::space_needed_for(vlen as u64) + vlen
            },
            &ValueLocation::Overflowed(vlen, ref blocks) => {
                1 + varint::space_needed_for(vlen as u64) + blocks.encoded_len()
            },
        }
    }

}

impl KeyLocation {
    fn need(&self, k: &[u8], prefixLen: usize) -> usize {
        let klen = k.len();
        match *self {
            KeyLocation::Inline => {
                0 // no key flags
                + varint::space_needed_for((klen * 2) as u64) 
                + klen 
                - prefixLen
            },
            KeyLocation::Overflowed(ref blocks) => {
                0 // no key flags
                + varint::space_needed_for((klen * 2 + 1) as u64) 
                + blocks.encoded_len()
            },
        }
    }
}

impl KeyWithLocation {
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    fn len_with_overflow_flag(&self) -> u64 {
        // inline key len is stored * 2, always an even number
        // overflowed key len is stored * 2 + 1, always odd
        match self.location {
            KeyLocation::Inline => {
                (self.key.len() * 2) as u64
            },
            KeyLocation::Overflowed(_) => {
                (self.key.len() * 2 + 1) as u64
            },
        }
    }

    fn need(&self, prefix_len: usize) -> usize {
        self.location.need(&self.key, prefix_len)
    }
}

#[derive(Debug)]
pub struct KeyWithLocation {
    key: Box<[u8]>,
    location: KeyLocation,
}

// this enum keeps track of what happened to a value as we
// processed it.  it might have already been overflowed.  if
// it's going to fit in the page, we still have the data
// buffer.
#[derive(Debug)]
enum ValueLocation {
    Tombstone,
    // when this is a Buffer, this gets ownership of kvp.Value
    Buffer(Box<[u8]>),
    Overflowed(usize, BlockList),
}

struct LeafPair {
    key: KeyWithLocation,
    value: ValueLocation,
}

impl LeafPair {
    fn need(&self, prefix_len: usize) -> usize {
        self.key.need(prefix_len) + self.value.need()
    }
}

struct LeafState {
    sofarLeaf: usize,
    keys_in_this_leaf: Vec<LeafPair>,
    prefixLen: usize,
    leaves: Vec<pgitem>,
}

fn write_overflow(
                    ba: &mut Read, 
                    pw: &mut PageWriter,
                    limit : usize,
                   ) -> Result<(usize, BlockList)> {

    fn write_page(ba: &mut Read, pb: &mut PageBuilder, pgsz: usize, pw: &mut PageWriter, blocks: &mut BlockList, limit: usize) -> Result<(usize, bool)> {
        pb.Reset();
        pb.PutByte(PageType::OVERFLOW_NODE.to_u8());
        let boundary = try!(pw.is_group_on_block_boundary(blocks, limit));
        let room = 
            if boundary.is_some() {
                pb.PutByte(PageFlag::FLAG_BOUNDARY_NODE);
                pgsz - 2 - SIZE_32
            } else {
                pb.PutByte(0u8);
                pgsz - 2
            };
        let put = try!(pb.PutStream2(ba, room));
        if put > 0 {
            if let Some(next_page) = boundary {
                pb.SetLastInt32(next_page);
            }
            try!(pw.write_group_page(pb.buf(), blocks, limit));
        } else {
            // there was nothing to write
        }
        Ok((put, put < room))
    };

    let pgsz = pw.page_size();
    let mut pb = PageBuilder::new(pgsz);

    let mut blocks = BlockList::new();
    let mut sofar = 0;
    loop {
        let (put, finished) = try!(write_page(ba, &mut pb, pgsz, pw, &mut blocks, limit));
        sofar += put;
        if finished {
            break;
        }
    }
    //println!("overflow: len = {}  blocks.len = {}  encoded_len: {}", sofar, blocks.len(), blocks.encoded_len());
    Ok((sofar, blocks))
}

fn write_leaves<I>(
                    pw: &mut PageWriter,
                    source: I,
                    ) -> Result<Vec<pgitem>> where I: Iterator<Item=Result<kvp>> {

    let mut pb = PageBuilder::new(pw.page_size());

    // TODO this is a buffer just for the purpose of being reused
    // in cases where the blob is provided as a stream, and we need
    // read a bit of it to figure out if it might fit inline rather
    // than overflow.
    let mut vbuf = vec![0; pw.page_size()].into_boxed_slice(); 

    // 2 for the page type and flags
    // 2 for the stored count
    const LEAF_PAGE_OVERHEAD: usize = 2 + 2;

    fn calc_page_len(prefix_len: usize, sofar: usize) -> usize {
        2 // page type and flags
        + 2 // stored count
        + varint::space_needed_for(prefix_len as u64)
        + prefix_len
        + sofar // sum of size of all the actual items
    }

    fn build_leaf(st: &mut LeafState, pb: &mut PageBuilder) -> (KeyWithLocation, Option<KeyWithLocation>, BlockList, usize) {
        pb.Reset();
        pb.PutByte(PageType::LEAF_NODE.to_u8());
        pb.PutByte(0u8); // flags
        pb.PutVarint(st.prefixLen as u64);
        if st.prefixLen > 0 {
            pb.PutArray(&st.keys_in_this_leaf[0].key.key[0 .. st.prefixLen]);
        }
        let count_keys_in_this_leaf = st.keys_in_this_leaf.len();
        // TODO should we support more than 64k keys in a leaf?
        // either way, overflow-check this cast.
        pb.PutInt16(count_keys_in_this_leaf as u16);

        fn f(pb: &mut PageBuilder, prefixLen: usize, lp: &LeafPair, list: &mut BlockList) {
            match lp.key.location {
                KeyLocation::Inline => {
                    pb.PutVarint(lp.key.len_with_overflow_flag());
                    pb.PutArray(&lp.key.key[prefixLen .. ]);
                },
                KeyLocation::Overflowed(ref blocks) => {
                    pb.PutVarint(lp.key.len_with_overflow_flag());
                    // TODO capacity, no temp vec
                    let mut v = vec![];
                    blocks.encode(&mut v);
                    pb.PutArray(&v);
                    list.add_blocklist_no_reorder(blocks);
                },
            }
            match lp.value {
                ValueLocation::Tombstone => {
                    pb.PutByte(ValueFlag::FLAG_TOMBSTONE);
                },
                ValueLocation::Buffer(ref vbuf) => {
                    pb.PutByte(0u8);
                    pb.PutVarint(vbuf.len() as u64);
                    pb.PutArray(&vbuf);
                },
                ValueLocation::Overflowed(vlen, ref blocks) => {
                    pb.PutByte(ValueFlag::FLAG_OVERFLOW);
                    pb.PutVarint(vlen as u64);
                    // TODO capacity, no temp vec
                    let mut v = vec![];
                    blocks.encode(&mut v);
                    pb.PutArray(&v);
                    list.add_blocklist_no_reorder(blocks);
                },
            }
        }

        let mut blocks = BlockList::new();

        // deal with the first key separately
        let first_key = {
            let lp = st.keys_in_this_leaf.remove(0); 
            assert!(st.keys_in_this_leaf.len() == count_keys_in_this_leaf - 1);
            f(pb, st.prefixLen, &lp, &mut blocks);
            lp.key
        };

        if st.keys_in_this_leaf.len() == 0 {
            // there was only one key in this leaf.
            blocks.sort_and_consolidate();
            (first_key, None, blocks, pb.sofar())
        } else {
            if st.keys_in_this_leaf.len() > 1 {
                // deal with all the remaining keys except the last one
                for lp in st.keys_in_this_leaf.drain(0 .. count_keys_in_this_leaf - 2) {
                    f(pb, st.prefixLen, &lp, &mut blocks);
                }
            }
            assert!(st.keys_in_this_leaf.len() == 1);

            // now the last key
            let last_key = {
                let lp = st.keys_in_this_leaf.remove(0); 
                assert!(st.keys_in_this_leaf.is_empty());
                f(pb, st.prefixLen, &lp, &mut blocks);
                lp.key
            };
            blocks.sort_and_consolidate();
            (first_key, Some(last_key), blocks, pb.sofar())
        }
    }

    fn write_leaf(st: &mut LeafState, 
                    pb: &mut PageBuilder, 
                    pw: &mut PageWriter,
                   ) -> Result<usize> {
        let (first_key, last_key, blocks, len_page) = build_leaf(st, pb);
        //println!("leaf blocklist: {:?}", blocks);
        //println!("leaf blocklist, len: {}   encoded_len: {:?}", blocks.len(), blocks.encoded_len());
        assert!(st.keys_in_this_leaf.is_empty());
        let thisPageNumber = try!(pw.write_page(pb.buf()));
        let pg = pgitem {
            page: thisPageNumber, 
            blocks: blocks,
            first_key: first_key,
            last_key: last_key
        };
        st.leaves.push(pg);
        st.sofarLeaf = 0;
        st.prefixLen = 0;
        Ok(len_page)
    }

    let pgsz = pw.page_size();

    let mut st = LeafState {
        sofarLeaf: 0,
        keys_in_this_leaf: Vec::new(),
        prefixLen: 0,
        leaves: Vec::new(),
    };

    let mut prev_key: Option<Box<[u8]>> = None;

    for result_pair in source {
        let mut pair = try!(result_pair);

        let k = pair.Key;
        if cfg!(expensive_check) 
        {
           // this code can be used to verify that we are being given kvps in order
            match prev_key {
                None => {
                },
                Some(prev_key) => {
                    let c = k.cmp(&prev_key);
                    if c != Ordering::Greater {
                        println!("prev_key: {:?}", prev_key);
                        println!("k: {:?}", k);
                        panic!("unsorted keys");
                    }
                },
            }
            prev_key = Some(k.clone());
        }

        let kloc = {
            // the max limit of an inline key is when that key is the only
            // one in the leaf, and its value is overflowed.  well actually,
            // the reference to an overflowed value can take up a few dozen bytes 
            // in pathological cases, whereas a really short value could be smaller.
            // nevermind that.

            // TODO the following code still needs tuning

            // TODO the following code is even more pessimistic now that we do
            // offsets in encoded blocklists
            const PESSIMISTIC_OVERFLOW_INFO_SIZE: usize = 
                1 // number of blocks
                +
                (
                5 // varint, block start page num, worst case
                + 4 // varint, num pages in block, pathological
                )
                * 8 // crazy case
                ;

            let maxKeyInline = 
                pgsz 
                - 2 // page type and flags
                - 2 // count keys
                - 1 // prefixLen of 0
                - varint::space_needed_for((pgsz * 2) as u64) // approx worst case inline key len
                - 1 // value flags
                - 9 // worst case varint value len
                - PESSIMISTIC_OVERFLOW_INFO_SIZE; // overflowed value page

            if k.len() <= maxKeyInline {
                KeyLocation::Inline
            } else {
                // for an overflowed key, there is probably no practical hard limit on the size
                // of the encoded block list.  we want things to be as contiguous as
                // possible, but if the block list won't fit on the usable part of a
                // fresh page, something is seriously wrong.
                // rather than calculate the actual hard limit, we provide an arbitrary
                // fraction of the page which would still be pathological.
                let hard_limit = pgsz / 4;
                let (len, blocks) = try!(write_overflow(&mut &*k, pw, hard_limit));
                assert!(len == k.len());
                KeyLocation::Overflowed(blocks)
            }
        };

        // we have decided whether the key is going to be inlined or overflowed.  but
        // we have NOT yet decided which page the key is going on.  will it fit on the
        // current page or does it have to bump to the next one?  we don't know yet.

        let vloc = {
            let maxValueInline = {
                // TODO use calc_page_len
                let fixed_costs_on_new_page =
                    LEAF_PAGE_OVERHEAD
                    + 2 // worst case prefixlen varint
                    + kloc.need(&k, 0)
                    + 1 // value flags
                    + varint::space_needed_for(pgsz as u64) // vlen can't be larger than this for inline
                    ;
                assert!(fixed_costs_on_new_page < pgsz);
                let available_for_inline_value_on_new_page = pgsz - fixed_costs_on_new_page;
                available_for_inline_value_on_new_page
            };

            // for the write_overflow calls below, we already know how much 
            // we spent on the key, so we know what our actual limit is for the encoded
            // block list for the value.  the hard limit, basically, is:  we cannot get
            // into a state where the key and value cannot fit on the same page.

            // TODO use calc_page_len
            let hard_limit_for_value_overflow =
                pgsz
                - LEAF_PAGE_OVERHEAD
                - 2 // worst case prefixlen varint
                - kloc.need(&k, 0)
                - 1 // value flags
                - 9 // worst case varint for value len
                ;

            match pair.Value {
                Blob::Tombstone => {
                    ValueLocation::Tombstone
                },
                Blob::Stream(ref mut strm) => {
                    // TODO
                    // not sure reusing vbuf is worth it.  maybe we should just
                    // alloc here.  ownership will get passed into the
                    // ValueLocation when it fits.
                    let vread = try!(misc::io::read_fully(&mut *strm, &mut vbuf[0 .. maxValueInline + 1]));
                    let vbuf = &vbuf[0 .. vread];
                    // TODO should be <= ?
                    if vread < maxValueInline {
                        // TODO this alloc+copy is unfortunate
                        let mut va = Vec::with_capacity(vbuf.len());
                        for i in 0 .. vbuf.len() {
                            va.push(vbuf[i]);
                        }
                        ValueLocation::Buffer(va.into_boxed_slice())
                    } else {
                        let (len, blocks) = try!(write_overflow(&mut (vbuf.chain(strm)), pw, hard_limit_for_value_overflow));
                        ValueLocation::Overflowed (len, blocks)
                    }
                },
                Blob::Array(a) => {
                    // TODO should be <= ?
                    if a.len() < maxValueInline {
                        ValueLocation::Buffer(a)
                    } else {
                        let (len, blocks) = try!(write_overflow(&mut &*a, pw, hard_limit_for_value_overflow));
                        ValueLocation::Overflowed(len, blocks)
                    }
                },
            }
        };

        // whether/not the key/value are to be overflowed is now already decided.
        // now all we have to do is decide if this key/value are going into this leaf
        // or not.  note that it is possible to overflow these and then have them not
        // fit into the current leaf and end up landing in the next leaf.

        fn calc_prefix_len(st: &LeafState, k: &[u8], kloc: &KeyLocation) -> usize {
            if st.keys_in_this_leaf.is_empty() {
                0
            } else {
                match kloc {
                    &KeyLocation::Inline => {
                        let max_prefix =
                            if st.prefixLen > 0 {
                                st.prefixLen
                            } else {
                                k.len()
                            };
                        bcmp::PrefixMatch(&*st.keys_in_this_leaf[0].key.key, &k, max_prefix)
                    },
                    &KeyLocation::Overflowed(_) => {
                        // an overflowed key does not change the prefix
                        st.prefixLen
                    },
                }
            }
        }

        let fits = {
            let would_be_prefix_len = calc_prefix_len(&st, &k, &kloc);
            let would_be_sofar = 
                if would_be_prefix_len != st.prefixLen {
                    assert!(st.prefixLen == 0 || would_be_prefix_len < st.prefixLen);
                    // the prefixLen would change with the addition of this key,
                    // so we need to recalc sofar
                    let sum = st.keys_in_this_leaf.iter().map(|lp| lp.need(would_be_prefix_len) ).sum();;
                    sum
                } else {
                    st.sofarLeaf
                };
            let would_be_len_page = calc_page_len(would_be_prefix_len, would_be_sofar);
            if pgsz > would_be_len_page {
                let available = pgsz - would_be_len_page;
                let needed = kloc.need(&k, would_be_prefix_len) + vloc.need();
                available >= needed
            } else {
                false
            }
        };

        if !fits {
            assert!(st.keys_in_this_leaf.len() > 0);
            let should_be = calc_page_len(st.prefixLen, st.sofarLeaf);
            let len_page = try!(write_leaf(&mut st, &mut pb, pw));
            //println!("should_be = {}   len_page = {}", should_be, len_page);
            assert!(should_be == len_page);
        }

        // see if the prefix len actually did change
        let new_prefix_len = calc_prefix_len(&st, &k, &kloc);
        let sofar = 
            if new_prefix_len != st.prefixLen {
                assert!(st.prefixLen == 0 || new_prefix_len < st.prefixLen);
                // the prefixLen will change with the addition of this key,
                // so we need to recalc sofar
                let sum = st.keys_in_this_leaf.iter().map(|lp| lp.need(new_prefix_len) ).sum();;
                sum
            } else {
                st.sofarLeaf
            };
        // note that the LeafPair struct gets ownership of the key provided
        // from above.
        let kwloc = KeyWithLocation {
            key: k,
            location: kloc,
        };
        let lp = LeafPair {
                    key: kwloc,
                    value: vloc,
                    };

        st.sofarLeaf = sofar + lp.need(new_prefix_len);
        st.prefixLen = new_prefix_len;
        st.keys_in_this_leaf.push(lp);
    }

    if !st.keys_in_this_leaf.is_empty() {
        assert!(st.keys_in_this_leaf.len() > 0);
        let should_be = calc_page_len(st.prefixLen, st.sofarLeaf);
        let len_page = try!(write_leaf(&mut st, &mut pb, pw));
        //println!("should_be = {}   len_page = {}", should_be, len_page);
        assert!(should_be == len_page);
    }
    Ok(st.leaves)
}

fn write_parent_node_tree(
                       children: Vec<pgitem>,
                       pw: &mut PageWriter,
                      ) -> Result<pgitem> {

    fn write_one_set_of_parent_nodes(
                           children: Vec<pgitem>,
                           pw: &mut PageWriter,
                           pb: &mut PageBuilder,
                          ) -> Result<Vec<pgitem>> {

        fn calc_page_len(prefix_len: usize, sofar: usize) -> usize {
            2 // page type and flags
            + 2 // stored count
            + varint::space_needed_for(prefix_len as u64) 
            + prefix_len 
            + sofar // sum of size of all the actual items
        }

        fn build_parent_page(st: &mut ParentState, 
                          pb: &mut PageBuilder,
                          ) -> (KeyWithLocation, Option<KeyWithLocation>, BlockList, usize) {
            // TODO? assert!(st.items.len() > 1);
            //println!("build_parent_page, prefixLen: {:?}", st.prefixLen);
            //println!("build_parent_page, items: {:?}", st.items);
            pb.Reset();
            pb.PutByte(PageType::PARENT_NODE.to_u8());
            pb.PutByte(0u8);
            pb.PutVarint(st.prefixLen as u64);
            if st.prefixLen > 0 {
                pb.PutArray(&st.items[0].first_key.key[0 .. st.prefixLen]);
            }
            pb.PutInt16(st.items.len() as u16);
            //println!("st.items.len(): {}", st.items.len());

            let mut list = BlockList::new();

            fn put_key(pb: &mut PageBuilder, k: &KeyWithLocation, prefix_len: usize) {
                match k.location {
                    KeyLocation::Inline => {
                        pb.PutArray(&k.key[prefix_len .. ]);
                    },
                    KeyLocation::Overflowed(ref blocks) => {
                        // TODO capacity, no temp vec
                        let mut v = vec![];
                        blocks.encode(&mut v);
                        pb.PutArray(&v);
                    },
                }
            }

            fn put_item(pb: &mut PageBuilder, list: &mut BlockList, item: &pgitem, prefix_len: usize) {
                pb.PutVarint(item.page as u64);
                list.add_page_no_reorder(item.page);
                //if cfg!(child_block_list) 
                {
                    // TODO capacity, no temp vec
                    let mut v = vec![];
                    item.blocks.encode(&mut v);
                    pb.PutArray(&v);
                }
                list.add_blocklist_no_reorder(&item.blocks);

                pb.PutVarint(item.first_key.len_with_overflow_flag());
                put_key(pb, &item.first_key, prefix_len);

                match item.last_key {
                    Some(ref last_key) => {
                        pb.PutVarint(last_key.len_with_overflow_flag());
                        put_key(pb, &last_key, prefix_len);
                    },
                    None => {
                        pb.PutVarint(0);
                    },
                }
            }

            // deal with the first item separately
            let (first_key, last_key_from_first_item) = {
                let item = st.items.remove(0); 
                put_item(pb, &mut list, &item, st.prefixLen);
                (item.first_key, item.last_key)
            };

            if st.items.len() == 0 {
                // there was only one item in this page
                list.sort_and_consolidate();
                (first_key, last_key_from_first_item, list, pb.sofar())
            } else {
                if st.items.len() > 1 {
                    // deal with all the remaining items except the last one
                    let tmp_count = st.items.len() - 1;
                    for item in st.items.drain(0 .. tmp_count) {
                        put_item(pb, &mut list, &item, st.prefixLen);
                    }
                }
                assert!(st.items.len() == 1);

                // now the last item
                let last_key = {
                    let item = st.items.remove(0); 
                    put_item(pb, &mut list, &item, st.prefixLen);
                    match item.last_key {
                        Some(last_key) => last_key,
                        None => item.first_key,
                    }
                };
                assert!(st.items.is_empty());

                list.sort_and_consolidate();
                (first_key, Some(last_key), list, pb.sofar())
            }
        }

        fn write_parent_page(st: &mut ParentState, 
                              pb: &mut PageBuilder, 
                              pw: &mut PageWriter,
                             ) -> Result<usize> {
            // assert st.sofar > 0
            let (first_key, last_key, blocks, len_page) = build_parent_page(st, pb);
            assert!(st.items.is_empty());
            //println!("parent blocklist: {:?}", blocks);
            //println!("parent blocklist, len: {}   encoded_len: {:?}", blocks.len(), blocks.encoded_len());
            let thisPageNumber = try!(pw.write_page(pb.buf()));
            let pg = pgitem {
                page: thisPageNumber, 
                blocks: blocks,
                first_key: first_key,
                last_key: last_key,
            };
            st.nextGeneration.push(pg);
            st.sofar = 0;
            st.prefixLen = 0;
            Ok(len_page)
        }

        let mut st = ParentState {
            nextGeneration: Vec::new(),
            prefixLen: 0,
            sofar: 0,
            items: vec![],
        };

        fn calc_prefix_len(st: &ParentState, item: &pgitem) -> usize {
            if st.items.is_empty() {
                match item.last_key {
                    Some(ref last_key) => {
                        bcmp::PrefixMatch(&*item.first_key.key, &last_key.key, last_key.key.len())
                    },
                    None => {
                        item.first_key.key.len()
                    },
                }
            } else {
                if st.prefixLen > 0 {
                    let a =
                        match &item.first_key.location {
                            &KeyLocation::Inline => {
                                bcmp::PrefixMatch(&*st.items[0].first_key.key, &item.first_key.key, st.prefixLen)
                            },
                            &KeyLocation::Overflowed(_) => {
                                // an overflowed key does not change the prefix
                                st.prefixLen
                            },
                        };
                    let b = 
                        match item.last_key {
                            Some(ref last_key) => {
                                match &last_key.location {
                                    &KeyLocation::Inline => {
                                        bcmp::PrefixMatch(&*st.items[0].first_key.key, &last_key.key, a)
                                    },
                                    &KeyLocation::Overflowed(_) => {
                                        // an overflowed key does not change the prefix
                                        a
                                    },
                                }
                            },
                            None => {
                                a
                            },
                        };
                    b
                } else {
                    0
                }
            }
        }

        let pgsz = pw.page_size();

        // deal with all the children except the last one
        for child in children {
            // to fit this child into this parent page, we need room for
            // the root page
            // the block list
            // prefixLen, varint
            // (prefix)
            // key len 1, varint, shifted for overflow flag
            // key 1
            //     if inline, the key, minus prefix
            //     if overflow, the blocklist (borrowed from child)
            // key len 2, varint, shifted for overflow flag, 0 if absent
            // key 2, if present
            //     if inline, the key, minus prefix
            //     if overflow, the blocklist (borrowed from child)

            // each key is stored just as it was in the child, inline or overflow.
            // if it is overflowed, we just store the same blocklist reference.

            // claim:  if both keys fit inlined in the child, they can both
            // fit inlined here in the parent page.

            let fits = {
                let would_be_prefix_len = calc_prefix_len(&st, &child);
                let would_be_sofar = 
                    if would_be_prefix_len != st.prefixLen {
                        assert!(st.prefixLen == 0 || would_be_prefix_len < st.prefixLen);
                        // the prefixLen would change with the addition of this key,
                        // so we need to recalc sofar
                        let sum = st.items.iter().map(|lp| lp.need(would_be_prefix_len) ).sum();;
                        sum
                    } else {
                        st.sofar
                    };
                let would_be_len_page = calc_page_len(would_be_prefix_len, would_be_sofar);
                if pgsz > would_be_len_page {
                    let available = pgsz - would_be_len_page;
                    available >= child.need(would_be_prefix_len)
                } else {
                    false
                }
            };

            if !fits {
                assert!(st.items.len() > 0);
                let should_be = calc_page_len(st.prefixLen, st.sofar);
                let len_page = try!(write_parent_page(&mut st, pb, pw));
                //println!("should_be = {}   len_page = {}", should_be, len_page);
                assert!(should_be == len_page);
                assert!(st.items.is_empty());
            }

            // see if the prefix len actually did change
            let new_prefix_len = calc_prefix_len(&st, &child);
            let sofar = 
                if new_prefix_len != st.prefixLen {
                    assert!(st.prefixLen == 0 || new_prefix_len < st.prefixLen);
                    // the prefixLen changed with the addition of this item,
                    // so we need to recalc sofar
                    let sum = st.items.iter().map(|lp| lp.need(new_prefix_len) ).sum();;
                    sum
                } else {
                    st.sofar
                };
            st.sofar = sofar + child.need(new_prefix_len);
            st.prefixLen = new_prefix_len;
            st.items.push(child);
        }

        {
            assert!(st.items.len() > 0);
            let should_be = calc_page_len(st.prefixLen, st.sofar);
            let len_page = try!(write_parent_page(&mut st, pb, pw));
            //println!("should_be = {}   len_page = {}", should_be, len_page);
            assert!(should_be == len_page);
            assert!(st.items.is_empty());
        }

        Ok(st.nextGeneration)
    }

    let mut pb = PageBuilder::new(pw.page_size());

    assert!(children.len() > 0);

    // all the leaves are written.
    // now write the parent pages.
    // maybe more than one level of them.
    // keep writing until we have written a level 
    // which has only one node,
    // which is the root node.

    let root_page =
        if children.len() > 1 {
            let mut kids = children;
            while kids.len() > 1 {
                // TODO FWIW, this is where we used to:
                // before we write this layer of parent nodes, we trim all the
                // keys to the shortest prefix that will suffice.

                let newChildren = try!(write_one_set_of_parent_nodes(kids, pw, &mut pb));
                kids = newChildren;
            }
            kids.remove(0)
        } else {
            let mut children = children;
            children.remove(0)
        };
    Ok(root_page)
}

fn create_segment<I>(mut pw: PageWriter, 
                                source: I,
                               ) -> Result<SegmentLocation> where I: Iterator<Item=Result<kvp>> {

    // TODO pw.reset_list() ?
    // or are we promised that the PageWriter we are given is empty?

    let leaves = try!(write_leaves(&mut pw, source));

    let root_page = try!(write_parent_node_tree(leaves, &mut pw));

    let blocks = try!(pw.end());

    if cfg!(expensive_check) 
    {
        // if we add the root page to the calculated block list 
        // it should be the same as the one from pw

        let mut tmp = root_page.blocks.clone();
        tmp.add_page_no_reorder(root_page.page);
        tmp.sort_and_consolidate();
        let mut v1 = vec![];
        tmp.encode(&mut v1);
        let mut v2 = vec![];
        blocks.encode(&mut v2);
        if v1 != v2 {
            println!("pw blocks: {:?}", blocks);
            println!("calc blocks: {:?}", tmp);
            assert!(v1 == v2);
        }
    }
    assert!(blocks.contains_page(root_page.page));
    let loc = SegmentLocation {
        root_page: root_page.page,
        blocks: blocks,
    };
    Ok(loc)
}

struct OverflowReader {
    fs: File,
    len: usize, // same type as ValueLength(), max len of a single value
    firstPage: PageNum, // TODO this will be needed later for Seek trait
    buf: Box<[u8]>,
    currentPage: PageNum,
    sofarOverall: usize,
    sofarThisPage: usize,
    bytesOnThisPage: usize,
    offsetOnThisPage: usize,
}
    
impl OverflowReader {
    fn new(path: &str, pgsz: usize, firstPage: PageNum, len: usize) -> Result<OverflowReader> {
        // TODO I wonder if maybe we should defer the opening of the file until
        // somebody actually tries to read from it?  so that constructing a
        // ValueRef object (which contains one of these) would be a lighter-weight
        // operation.
        let f = try!(OpenOptions::new()
                .read(true)
                .open(path));
        let mut res = 
            // TODO the vec new below is really slow.  use a pool?
            OverflowReader {
                fs: f,
                len: len,
                firstPage: firstPage,
                buf: vec![0; pgsz].into_boxed_slice(),
                currentPage: firstPage,
                sofarOverall: 0,
                sofarThisPage: 0,
                bytesOnThisPage: 0,
                offsetOnThisPage: 0,
            };
        try!(res.ReadPage());
        Ok(res)
    }

    // TODO consider supporting Seek trait

    fn ReadPage(&mut self) -> Result<()> {
        try!(utils::SeekPage(&mut self.fs, self.buf.len(), self.currentPage));
        try!(misc::io::read_fully(&mut self.fs, &mut *self.buf));
        if try!(self.PageType()) != (PageType::OVERFLOW_NODE) {
            return Err(Error::CorruptFile("first overflow page has invalid page type"));
        }
        self.sofarThisPage = 0;
        self.offsetOnThisPage = 2;
        if self.CheckPageFlag(PageFlag::FLAG_BOUNDARY_NODE) {
            self.bytesOnThisPage = self.buf.len() - 2 - SIZE_32;
        } else {
            self.bytesOnThisPage = self.buf.len() - 2;
        }
        Ok(())
    }

    fn GetLastInt32(&self) -> u32 {
        let at = self.buf.len() - SIZE_32;
        // TODO just self.buf?  instead of making 4-byte slice.
        let a = misc::bytes::extract_4(&self.buf[at .. at + 4]);
        endian::u32_from_bytes_be(a)
    }

    fn PageType(&self) -> Result<PageType> {
        PageType::from_u8(self.buf[0])
    }

    fn CheckPageFlag(&self, f: u8) -> bool {
        0 != (self.buf[1] & f)
    }

    fn Read(&mut self, ba: &mut [u8], offset: usize, wanted: usize) -> Result<usize> {
        if self.sofarOverall >= self.len {
            Ok(0)
        } else {
            if self.sofarThisPage >= self.bytesOnThisPage {
                if self.CheckPageFlag(PageFlag::FLAG_BOUNDARY_NODE) {
                    self.currentPage = self.GetLastInt32();
                } else {
                    self.currentPage += 1;
                }
                try!(self.ReadPage());
            }

            let available = std::cmp::min(self.bytesOnThisPage - self.sofarThisPage, self.len - self.sofarOverall);
            let num = std::cmp::min(available, wanted);
            for i in 0 .. num {
                ba[offset + i] = self.buf[self.offsetOnThisPage + self.sofarThisPage + i];
            }
            self.sofarOverall = self.sofarOverall + num;
            self.sofarThisPage = self.sofarThisPage + num;
            Ok(num)
        }
    }
}

impl Read for OverflowReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len();
        match self.Read(buf, 0, len) {
            Ok(v) => Ok(v),
            Err(e) => {
                // this interface requires io::Result, so we shoehorn the others into it
                match e {
                    Error::Io(e) => Err(e),
                    _ => {
                        use std::error::Error;
                        Err(std::io::Error::new(std::io::ErrorKind::Other, e.description()))
                    }
                }
            },
        }
    }
}

pub struct LeafPage {
    path: String,
    f: std::rc::Rc<std::cell::RefCell<File>>,
    pr: Lend<Box<[u8]>>,
    pagenum: PageNum, // TODO diag only?

    prefix: Option<Box<[u8]>>,

    pairs: Vec<PairInLeaf>,
}

impl LeafPage {
    fn new_already_read_page(path: &str, 
           f: std::rc::Rc<std::cell::RefCell<File>>,
           pagenum: PageNum,
           buf: Lend<Box<[u8]>>
          ) -> Result<LeafPage> {

        let mut res = LeafPage {
            path: String::from(path),
            f: f,
            pagenum: pagenum,
            pr: buf,
            pairs: Vec::new(),
            prefix: None,
        };

        try!(res.parse_page());

        Ok(res)
    }

    fn count_keys(&self) -> usize {
        self.pairs.len()
    }

    // TODO not sure if we need to provide funcs: key_overflows() and value_overflows()

    pub fn overflows(&self) -> Vec<&BlockList> {
        let mut list = vec![];
        for i in 0 .. self.pairs.len() {
            match &self.pairs[i].key {
                &KeyInPage::Inline(_, _) => {
                },
                &KeyInPage::Overflowed(_, ref blocks) => {
                    list.push(blocks);
                },
            }
            match &self.pairs[i].value {
                &ValueInLeaf::Tombstone => {
                },
                &ValueInLeaf::Inline(_, _) => {
                },
                &ValueInLeaf::Overflowed(_, ref blocks) => {
                    list.push(blocks);
                },
            }
        }
        list
    }

    pub fn complete_blocklist(&self) -> BlockList {
        let mut list = BlockList::new();
        for blist in self.overflows() {
            list.add_blocklist_no_reorder(blist);
        }
        list.sort_and_consolidate();
        list
    }

    fn parse_page(&mut self) -> Result<()> {
        self.prefix = None;

        let mut cur = 0;
        let pt = try!(PageType::from_u8(misc::buf_advance::get_byte(&self.pr, &mut cur)));
        if pt != PageType::LEAF_NODE {
            return Err(Error::CorruptFile("leaf has invalid page type"));
        }
        cur = cur + 1; // skip flags

        // TODO fn read_prefix_from_page()
        let prefix_len = varint::read(&self.pr, &mut cur) as usize;
        if prefix_len > 0 {
            // TODO should we just remember prefix as a reference instead of box/copy?
            let mut a = vec![0; prefix_len].into_boxed_slice();
            a.clone_from_slice(&self.pr[cur .. cur + prefix_len]);
            cur = cur + prefix_len;
            self.prefix = Some(a);
        } else {
            self.prefix = None;
        }
        let count_keys = misc::buf_advance::get_u16(&self.pr, &mut cur) as usize;
        // assert count_keys>0

        match self.pairs.len().cmp(&count_keys) {
            Ordering::Equal => {
            },
            Ordering::Less => {
                self.pairs.reserve(count_keys);
            },
            Ordering::Greater => {
                self.pairs.truncate(count_keys);
            },
        }

        for i in 0 .. count_keys {
            let k = try!(KeyInPage::read(&self.pr, &mut cur, prefix_len));
            let v = try!(ValueInLeaf::read(&self.pr, &mut cur));
            let pair = PairInLeaf {
                key: k,
                value: v,
            };

            if i < self.pairs.len() {
                self.pairs[i] = pair;
            } else {
                self.pairs.push(pair);
            }
        }

        Ok(())
    }

    fn read_page(&mut self, pgnum: PageNum) -> Result<()> {
        {
            let f = &mut *(self.f.borrow_mut());
            try!(utils::SeekPage(f, self.pr.len(), pgnum));
            try!(misc::io::read_fully(f, &mut self.pr));
            self.pagenum = pgnum;
        }
        try!(self.parse_page());
        Ok(())
    }

    // TODO shouldn't we have a method that returns the KeyInLeaf?
    #[inline]
    fn key<'a>(&'a self, n: usize) -> Result<KeyRef<'a>> { 
        let prefix: Option<&[u8]> = 
            match self.prefix {
                Some(ref b) => Some(b),
                None => None,
            };
        let k = try!(self.pairs[n].key.keyref(&self.pr, prefix, &self.path));
        Ok(k)
    }

    // TODO shouldn't we have a method that returns the ValueInLeaf?
    fn value<'a>(&'a self, n: usize) -> Result<ValueRef<'a>> {
        match &self.pairs[n].value {
            &ValueInLeaf::Tombstone => {
                Ok(ValueRef::Tombstone)
            },
            &ValueInLeaf::Inline(vlen, pos) => {
                Ok(ValueRef::Array(&self.pr[pos .. pos + vlen]))
            },
            &ValueInLeaf::Overflowed(vlen, ref blocks) => {
                // TODO return blocks instead of opening the stream
                let strm = try!(OverflowReader::new(&self.path, self.pr.len(), blocks.blocks[0].firstPage, vlen));
                Ok(ValueRef::Overflowed(vlen, box strm))
            },
        }
    }

    // TODO shouldn't we have a method that returns the ValueInLeaf?
    fn value_len<'a>(&'a self, n: usize) -> Result<Option<usize>> {
        match &self.pairs[n].value {
            &ValueInLeaf::Tombstone => {
                Ok(None)
            },
            &ValueInLeaf::Inline(vlen, _) => {
                Ok(Some(vlen))
            },
            &ValueInLeaf::Overflowed(vlen, _) => {
                Ok(Some(vlen))
            },
        }
    }

    fn search(&self, k: &KeyRef, min: usize, max: usize, sop: SeekOp, le: Option<usize>, ge: Option<usize>) -> Result<(Option<usize>, bool)> {
        if max < min {
            match sop {
                SeekOp::SEEK_EQ => Ok((None, false)),
                SeekOp::SEEK_LE => Ok((le, false)),
                SeekOp::SEEK_GE => Ok((ge, false)),
            }
        } else {
            let mid = (max + min) / 2;
            // assert mid >= 0
            let cmp = {
                let q = try!(self.key(mid));
                KeyRef::cmp(&q, k)
            };
            match cmp {
                Ordering::Equal => Ok((Some(mid), true)),
                Ordering::Less => self.search(k, (mid+1), max, sop, Some(mid), ge),
                Ordering::Greater => 
                    // we could just recurse with mid-1, but that would overflow if
                    // mod is 0, so we catch that case here.
                    if mid == 0 { 
                        match sop {
                            SeekOp::SEEK_EQ => Ok((None, false)),
                            SeekOp::SEEK_LE => Ok((le, false)),
                            SeekOp::SEEK_GE => Ok((Some(mid), false)),
                        }
                    } else { 
                        self.search(k, min, (mid-1), sop, le, Some(mid))
                    },
            }
        }
    }

}

pub struct LeafCursor {
    page: LeafPage,
    cur: Option<usize>,
}

impl LeafCursor {
    fn new(page: LeafPage) -> LeafCursor {
        LeafCursor {
            page: page,
            cur: None,
        }
    }

    fn seek(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        let tmp_countLeafKeys = self.page.count_keys();
        let (newCur, equal) = try!(self.page.search(k, 0, (tmp_countLeafKeys - 1), sop, None, None));
        self.cur = newCur;
        if self.cur.is_none() {
            Ok(SeekResult::Invalid)
        } else if equal {
            Ok(SeekResult::Equal)
        } else {
            Ok(SeekResult::Unequal)
        }
    }

    fn read_page(&mut self, pgnum: PageNum) -> Result<()> {
        self.page.read_page(pgnum)
    }
}

impl ICursor for LeafCursor {
    fn IsValid(&self) -> bool {
        if let Some(i) = self.cur {
            assert!(i < self.page.count_keys());
            true
        } else {
            false
        }
    }

    fn SeekRef(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        //println!("Leaf SeekRef {}  k={:?}  sop={:?}", self.pagenum, k, sop);
        let sr = try!(self.seek(k, sop));
        if cfg!(expensive_check) 
        {
            try!(sr.verify(k, sop, self));
        }
        Ok(sr)
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(cur) => {
                self.page.key(cur)
            },
        }
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(cur) => {
                self.page.value(cur)
            }
        }
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(cur) => {
                self.page.value_len(cur)
            }
        }
    }

    fn First(&mut self) -> Result<()> {
        self.cur = Some(0);
        Ok(())
    }

    fn Last(&mut self) -> Result<()> {
        self.cur = Some(self.page.count_keys() - 1);
        Ok(())
    }

    fn Next(&mut self) -> Result<()> {
        match self.cur {
            Some(cur) => {
                if (cur + 1) < self.page.count_keys() {
                    self.cur = Some(cur + 1);
                } else {
                    self.cur = None;
                }
            },
            None => {
            },
        }
        Ok(())
    }

    fn Prev(&mut self) -> Result<()> {
        match self.cur {
            Some(cur) => {
                if cur > 0 {
                    self.cur = Some(cur - 1);
                } else {
                    self.cur = None;
                }
            },
            None => {
            },
        }
        Ok(())
    }

}

pub enum PageCursor {
    Leaf(LeafCursor),
    Parent(ParentCursor),
}

impl PageCursor {
    fn new(path: &str, 
           f: std::rc::Rc<std::cell::RefCell<File>>,
           pagenum: PageNum,
           mut buf: Lend<Box<[u8]>>
          ) -> Result<PageCursor> {

        {
            let f = &mut *(f.borrow_mut());
            try!(utils::SeekPage(f, buf.len(), pagenum));
            try!(misc::io::read_fully(f, &mut buf));
        }

        let pt = try!(PageType::from_u8(buf[0]));
        let sub = 
            match pt {
                PageType::LEAF_NODE => {
                    let page = try!(LeafPage::new_already_read_page(path, f, pagenum, buf));
                    let sub = LeafCursor::new(page);
                    PageCursor::Leaf(sub)
                },
                PageType::PARENT_NODE => {
                    let page = try!(ParentPage::new_already_read_page(path, f, pagenum, buf));
                    let sub = try!(ParentCursor::new(page));
                    PageCursor::Parent(sub)
                },
                PageType::OVERFLOW_NODE => {
                    return Err(Error::CorruptFile("child page has invalid page type"));
                },
            };

        Ok(sub)
    }

    pub fn page_type(&self) -> PageType {
        match self {
            &PageCursor::Leaf(_) => {
                PageType::LEAF_NODE
            },
            &PageCursor::Parent(_) => {
                PageType::PARENT_NODE
            },
        }
    }

    fn read_page(&mut self, pg: PageNum) -> Result<()> {
        match self {
            &mut PageCursor::Leaf(ref mut c) => {
                try!(c.read_page(pg));
            },
            &mut PageCursor::Parent(ref mut c) => {
                try!(c.read_page(pg));
            },
        }
        Ok(())
    }
}

impl ICursor for PageCursor {
    fn IsValid(&self) -> bool {
        match self {
            &PageCursor::Leaf(ref c) => c.IsValid(),
            &PageCursor::Parent(ref c) => c.IsValid(),
        }
    }

    fn SeekRef(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        //println!("PageCursor SeekRef  k={:?}  sop={:?}", k, sop);
        let sr = 
            match self {
                &mut PageCursor::Leaf(ref mut c) => c.SeekRef(k, sop),
                &mut PageCursor::Parent(ref mut c) => c.SeekRef(k, sop),
            };
        let sr = try!(sr);
        if cfg!(expensive_check) 
        {
            try!(sr.verify(k, sop, self));
        }
        Ok(sr)
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        match self {
            &PageCursor::Leaf(ref c) => c.KeyRef(),
            &PageCursor::Parent(ref c) => c.KeyRef(),
        }
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        match self {
            &PageCursor::Leaf(ref c) => c.ValueRef(),
            &PageCursor::Parent(ref c) => c.ValueRef(),
        }
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        match self {
            &PageCursor::Leaf(ref c) => c.ValueLength(),
            &PageCursor::Parent(ref c) => c.ValueLength(),
        }
    }

    fn First(&mut self) -> Result<()> {
        match self {
            &mut PageCursor::Leaf(ref mut c) => c.First(),
            &mut PageCursor::Parent(ref mut c) => c.First(),
        }
    }

    fn Last(&mut self) -> Result<()> {
        match self {
            &mut PageCursor::Leaf(ref mut c) => c.Last(),
            &mut PageCursor::Parent(ref mut c) => c.Last(),
        }
    }

    fn Next(&mut self) -> Result<()> {
        match self {
            &mut PageCursor::Leaf(ref mut c) => c.Next(),
            &mut PageCursor::Parent(ref mut c) => c.Next(),
        }
    }

    fn Prev(&mut self) -> Result<()> {
        match self {
            &mut PageCursor::Leaf(ref mut c) => c.Prev(),
            &mut PageCursor::Parent(ref mut c) => c.Prev(),
        }
    }

}

#[derive(Debug)]
pub enum KeyInPage {
    Inline(usize, usize),
    Overflowed(usize, BlockList),
}

impl KeyInPage {
    #[inline]
    fn read(pr: &[u8], cur: &mut usize, prefix_len: usize) -> Result<Self> {
        let k = try!(Self::read_optional(pr, cur, prefix_len));
        match k {
            Some(k) => Ok(k),
            None => Err(Error::CorruptFile("key cannot be zero length")),
        }
    }

    #[inline]
    fn read_optional(pr: &[u8], cur: &mut usize, prefix_len: usize) -> Result<Option<Self>> {
        let klen = varint::read(pr, cur) as usize;
        if klen == 0 {
            return Ok(None);
        }
        let inline = 0 == klen % 2;
        let klen = klen / 2;
        let k = 
            if inline {
                let k = KeyInPage::Inline(klen, *cur);
                *cur += klen - prefix_len;
                k
            } else {
                let blocks = BlockList::read(&pr, cur);
                let k = KeyInPage::Overflowed(klen, blocks);
                k
            };
        Ok(Some(k))
    }

    #[inline]
    fn keyref<'a>(&self, pr: &'a [u8], prefix: Option<&'a [u8]>, path: &str) -> Result<KeyRef<'a>> { 
        match self {
            &KeyInPage::Inline(klen, at) => {
                match prefix {
                    Some(a) => {
                        Ok(KeyRef::Prefixed(&a, &pr[at .. at + klen - a.len()]))
                    },
                    None => {
                        Ok(KeyRef::Array(&pr[at .. at + klen]))
                    },
                }
            },
            &KeyInPage::Overflowed(klen, ref blocks) => {
                let mut ostrm = try!(OverflowReader::new(path, pr.len(), blocks.blocks[0].firstPage, klen));
                let mut x_k = Vec::with_capacity(klen);
                try!(ostrm.read_to_end(&mut x_k));
                let x_k = x_k.into_boxed_slice();
                Ok(KeyRef::Boxed(x_k))
            },
        }
    }

}

#[derive(Debug)]
enum ValueInLeaf {
    Tombstone,
    Inline(usize, usize),
    Overflowed(usize, BlockList),
}

impl ValueInLeaf {
    fn read(pr: &[u8], cur: &mut usize) -> Result<Self> {
        let vflag = misc::buf_advance::get_byte(pr, cur);
        let v = 
            if 0 != (vflag & ValueFlag::FLAG_TOMBSTONE) { 
                ValueInLeaf::Tombstone
            } else {
                let vlen = varint::read(pr, cur) as usize;
                if 0 != (vflag & ValueFlag::FLAG_OVERFLOW) {
                    let blocks = BlockList::read(&pr, cur);
                    ValueInLeaf::Overflowed(vlen, blocks)
                } else {
                    let v = ValueInLeaf::Inline(vlen, *cur);
                    *cur = *cur + vlen;
                    v
                }
            };
        Ok(v)
    }
}

#[derive(Debug)]
struct PairInLeaf {
    key: KeyInPage,
    value: ValueInLeaf,
}

#[derive(Debug)]
pub struct ParentPageItem {
    page: PageNum,

    //if cfg!(child_block_list) 
    blocks: BlockList,

    // blocks does NOT contain page

    first_key: KeyInPage,
    last_key: Option<KeyInPage>,
}

#[derive(Debug)]
enum KeyInRange {
    Less,
    Greater,
    EqualFirst,
    EqualLast,
    Within,
}

impl ParentPageItem {
    pub fn page(&self) -> PageNum {
        self.page
    }

    pub fn first_key(&self) -> &KeyInPage {
        &self.first_key
    }

    pub fn last_key(&self) -> &Option<KeyInPage> {
        &self.last_key
    }

}

pub struct ParentPage {
    path: String,
    f: std::rc::Rc<std::cell::RefCell<File>>,
    pr: Lend<Box<[u8]>>,
    pagenum: PageNum, // TODO diag only?

    prefix: Option<Box<[u8]>>,
    children: Vec<ParentPageItem>,

}

impl ParentPage {
    fn new_already_read_page(path: &str, 
           f: std::rc::Rc<std::cell::RefCell<File>>,
           pagenum: PageNum,
           buf: Lend<Box<[u8]>>
          ) -> Result<ParentPage> {

        let (prefix, children) = try!(Self::parse_page(&buf));
        assert!(children.len() > 0);

        let res = ParentPage {
            path: String::from(path),
            f: f,
            pr: buf,
            pagenum: pagenum,

            prefix: prefix,
            children: children,

        };

        if cfg!(expensive_check) 
        {
            try!(res.verify_child_keys());
        }

        Ok(res)
    }

    pub fn complete_blocklist(&self) -> BlockList {
        let mut list = BlockList::new();
        for i in 0 .. self.children.len() {
            // we do not add the blocklist for any overflow keys,
            // because we don't own that blocklist.  it is simply a reference
            // to the blocklist for an overflow key when it was written into
            // its leaf.
            list.add_page_no_reorder(self.children[i].page);
            list.add_blocklist_no_reorder(&self.children[i].blocks);
        }
        list.sort_and_consolidate();
        list
    }

    pub fn count_items(&self) -> usize {
        self.children.len()
    }

    fn verify_child_keys(&self) -> Result<()> {
        for i in 0 .. self.children.len() {
            // for each child, make sure the last key > first key
            match &self.children[i].last_key {
                &Some(ref last_key) => {
                    let first_key = try!(self.key(&self.children[i].first_key));
                    let last_key = try!(self.key(last_key));
                    let c = KeyRef::cmp(&last_key, &first_key);
                    assert!(c == Ordering::Greater);
                },
                &None => {
                },
            };

            // for each child, make sure it provides the same first and
            // last key as the ones we've got.

            // TODO it's a little silly here to construct a Lend<>
            let child_buf = vec![0; self.pr.len()].into_boxed_slice();
            let done_page = move |_| -> () {
            };
            let mut child_buf = Lend::new(child_buf, box done_page);
            let mut sub = try!(PageCursor::new(&self.path, self.f.clone(), self.children[i].page, child_buf));
            try!(sub.First());
            assert!(sub.IsValid());
            {
                let k = try!(sub.KeyRef());
                let first_key = try!(self.key(&self.children[i].first_key));
                let c = KeyRef::cmp(&k, &first_key);
                assert!(c == Ordering::Equal);
            }
            try!(sub.Last());
            assert!(sub.IsValid());
            let last_key =
                match &self.children[i].last_key {
                    &Some(ref last_key) => {
                        last_key
                    },
                    &None => {
                        &self.children[i].first_key
                    },
                };
            {
                let k = try!(sub.KeyRef());
                let last_key = try!(self.key(last_key));
                let c = KeyRef::cmp(&k, &last_key);
                assert!(c == Ordering::Equal);
            }
        }
        Ok(())
    }

    #[inline]
    fn key<'a>(&'a self, k: &KeyInPage) -> Result<KeyRef<'a>> { 
        let prefix: Option<&[u8]> = 
            match self.prefix {
                Some(ref b) => Some(b),
                None => None,
            };
        let k = try!(k.keyref(&self.pr, prefix, &self.path));
        Ok(k)
    }

    fn parse_page(pr: &[u8]) -> Result<(Option<Box<[u8]>>, Vec<ParentPageItem>)> {
        let mut cur = 0;
        let pt = try!(PageType::from_u8(misc::buf_advance::get_byte(pr, &mut cur)));
        if  pt != PageType::PARENT_NODE {
            return Err(Error::CorruptFile("parent page has invalid page type"));
        }
        cur = cur + 1; // skip the page flags
        let prefix_len = varint::read(pr, &mut cur) as usize;
        let prefix = {
            if prefix_len > 0 {
                // TODO should we just remember prefix as a reference instead of box/copy?
                let mut a = vec![0; prefix_len].into_boxed_slice();
                a.clone_from_slice(&pr[cur .. cur + prefix_len]);
                cur = cur + prefix_len;
                Some(a)
            } else {
                None
            }
        };
        let count_items = misc::buf_advance::get_u16(pr, &mut cur) as usize;
        // TODO count_items has gotta be > 1, right?  or does it?
        //println!("count_items: {}", count_items);
        let mut children = Vec::with_capacity(count_items);

        for i in 0 .. count_items {
            let page = varint::read(pr, &mut cur) as PageNum;
            //println!("page: {}", page);

            //if cfg!(child_block_list) 
            let blocks = BlockList::read(pr, &mut cur);

            let key_1 = try!(KeyInPage::read(pr, &mut cur, prefix_len));
            let key_2 = try!(KeyInPage::read_optional(pr, &mut cur, prefix_len));

            let pg = ParentPageItem {
                page: page, 
                //if cfg!(child_block_list) 
                blocks: blocks,
                first_key: key_1,
                last_key: key_2,
            };
            children.push(pg);
        }

        //println!("children parsed: {:?}", children);

        Ok((prefix, children))
    }

    fn read_page(&mut self, pgnum: PageNum) -> Result<()> {
        {
            let f = &mut *(self.f.borrow_mut());
            try!(utils::SeekPage(f, self.pr.len(), pgnum));
            try!(misc::io::read_fully(f, &mut self.pr));
            self.pagenum = pgnum;
        }
        let (prefix, children) = try!(Self::parse_page(&self.pr));
        self.prefix = prefix;
        self.children = children;

        Ok(())
    }

    fn key_in_child_range(&self, i: usize, kq: &KeyRef) -> Result<KeyInRange> {
        match self.children[i].last_key {
            Some(ref last_key) => {
                let last_key = try!(self.key(last_key));
                match KeyRef::cmp(kq, &last_key) {
                    Ordering::Less => {
                    },
                    Ordering::Equal => {
                        return Ok(KeyInRange::EqualLast);
                    },
                    Ordering::Greater => {
                        return Ok(KeyInRange::Greater);
                    },
                }
            },
            None => {
            },
        }
        let first_key = try!(self.key(&self.children[i].first_key));
        match KeyRef::cmp(kq, &first_key) {
            Ordering::Less => {
                Ok(KeyInRange::Less)
            },
            Ordering::Equal => {
                Ok(KeyInRange::EqualFirst)
            },
            Ordering::Greater => {
                if self.children[i].last_key.is_some() {
                    Ok(KeyInRange::Within)
                } else {
                    Ok(KeyInRange::Greater)
                }
            },
        }
    }

    fn get_child_cursor(&self, i: usize) -> Result<PageCursor> {
        // TODO it's a little silly here to construct a Lend<>
        let child_buf = vec![0; self.pr.len()].into_boxed_slice();
        let done_page = move |_| -> () {
        };
        let mut child_buf = Lend::new(child_buf, box done_page);

        let sub = try!(PageCursor::new(&self.path, self.f.clone(), self.children[i].page, child_buf));
        Ok(sub)
    }

    fn get_child_pagenum(&self, i:usize) -> PageNum {
        self.children[i].page
    }
}

pub struct ParentCursor {
    page: ParentPage,
    cur: Option<usize>,
    sub: Box<PageCursor>,
}

impl ParentCursor {
    fn new(page: ParentPage,) -> Result<ParentCursor> {

        let sub = try!(page.get_child_cursor(0));

        let res = ParentCursor {
            page: page,
            cur: Some(0),
            sub: box sub,
        };

        Ok(res)
    }

    pub fn child_page_type(&self) -> PageType {
        self.sub.page_type()
    }

    fn set_child(&mut self, i: usize) -> Result<()> {
        let pagenum = self.page.get_child_pagenum(i);
        try!(self.sub.read_page(pagenum));
        self.cur = Some(i);
        Ok(())
    }

    fn read_page(&mut self, pgnum: PageNum) -> Result<()> {
        self.page.read_page(pgnum)
    }

    fn seek(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        //println!("parent page: {}  search: kq = {:?},  sop = {:?}", self.pagenum, k, sop);

        for i in 0 .. self.page.count_items() {
            let look = try!(self.page.key_in_child_range(i, k));
            //println!("    in page {:?} : {:?}", self.children[i].page, look);
            match look {
                KeyInRange::EqualFirst => {
                    try!(self.set_child(i));
                    try!(self.sub.First());
                    // TODO assert something?
                    return Ok(SeekResult::Equal);
                },
                KeyInRange::EqualLast => {
                    try!(self.set_child(i));
                    try!(self.sub.Last());
                    // TODO assert something?
                    return Ok(SeekResult::Equal);
                },
                KeyInRange::Within => {
                    try!(self.set_child(i));
                    let sr = try!(self.sub.SeekRef(k, sop));
                    return Ok(sr)
                },
                KeyInRange::Less => {
                    // there should be no case here where the loop continues.
                    // the children are sorted ascending.  if the key is
                    // less than this one, it will be less than all the others
                    // as well.
                    if i == 0 {
                        // k is less than the first one
                        match sop {
                            SeekOp::SEEK_LE => {
                                self.cur = None;
                                return Ok(SeekResult::Invalid);
                            },
                            SeekOp::SEEK_GE => {
                                try!(self.set_child(0));
                                try!(self.sub.First());
                                return Ok(SeekResult::Unequal);
                            },
                            SeekOp::SEEK_EQ => {
                                self.cur = None;
                                return Ok(SeekResult::Invalid);
                            },
                        }
                    } else {
                        // had to be greater than the last one
                        // in between
                        match sop {
                            SeekOp::SEEK_LE => {
                                try!(self.set_child(i - 1));
                                try!(self.sub.Last());
                                return Ok(SeekResult::Unequal);
                            },
                            SeekOp::SEEK_GE => {
                                try!(self.set_child(i));
                                try!(self.sub.First());
                                return Ok(SeekResult::Unequal);
                            },
                            SeekOp::SEEK_EQ => {
                                self.cur = None;
                                return Ok(SeekResult::Invalid);
                            },
                        }
                    }
                },
                KeyInRange::Greater => {
                    // keep looking
                },
            }
        }
        //println!("after loop");
        // the only way to exit the loop is for the key to be
        // greater than the last item.
        match sop {
            SeekOp::SEEK_LE => {
                let last_child = self.page.count_items() - 1;
                try!(self.set_child(last_child));
                try!(self.sub.Last());
                return Ok(SeekResult::Unequal);
            },
            SeekOp::SEEK_GE => {
                self.cur = None;
                return Ok(SeekResult::Invalid);
            },
            SeekOp::SEEK_EQ => {
                self.cur = None;
                return Ok(SeekResult::Invalid);
            },
        }
    }

}

impl ICursor for ParentCursor {
    fn IsValid(&self) -> bool {
        match self.cur {
            None => false,
            Some(_) => {
                self.sub.IsValid()
            },
        }
    }

    fn SeekRef(&mut self, k: &KeyRef, sop: SeekOp) -> Result<SeekResult> {
        let sr = try!(self.seek(k, sop));
        if cfg!(expensive_check) 
        {
            try!(sr.verify(k, sop, self));
        }
        Ok(sr)
    }

    fn KeyRef<'a>(&'a self) -> Result<KeyRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(_) => {
                self.sub.KeyRef()
            },
        }
    }

    fn ValueRef<'a>(&'a self) -> Result<ValueRef<'a>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(_) => {
                self.sub.ValueRef()
            },
        }
    }

    fn ValueLength(&self) -> Result<Option<usize>> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(_) => {
                self.sub.ValueLength()
            },
        }
    }

    fn First(&mut self) -> Result<()> {
        try!(self.set_child(0));
        self.sub.First()
    }

    fn Last(&mut self) -> Result<()> {
        let last_child = self.page.count_items() - 1;
        try!(self.set_child(last_child));
        self.sub.Last()
    }

    fn Next(&mut self) -> Result<()> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(i) => {
                try!(self.sub.Next());
                if !self.sub.IsValid() && i + 1 < self.page.count_items() {
                    try!(self.set_child(i + 1));
                    self.sub.First()
                } else {
                    Ok(())
                }
            },
        }
    }

    fn Prev(&mut self) -> Result<()> {
        match self.cur {
            None => {
                Err(Error::CursorNotValid)
            },
            Some(i) => {
                try!(self.sub.Prev());
                if !self.sub.IsValid() && i > 0 {
                    try!(self.set_child(i - 1));
                    self.sub.Last()
                } else {
                    Ok(())
                }
            },
        }
    }

}

#[derive(Clone)]
struct HeaderData {
    // TODO young, levels, etc
    current_state: Vec<SegmentNum>,

    segments_info: HashMap<SegmentNum, SegmentInfo>,

    // TODO maybe the locations don't need to be in the
    // header?

    next_segnum: SegmentNum,
    changeCounter: u64,
    mergeCounter: u64,
}

// TODO how big should the header be?  this defines the minimum size of a
// database file.
const HEADER_SIZE_IN_BYTES: usize = 4096;

fn read_header(path: &str) -> Result<(HeaderData, usize, PageNum)> {
    fn read<R>(fs: &mut R) -> Result<Box<[u8]>> where R : Read {
        let mut pr = vec![0; HEADER_SIZE_IN_BYTES].into_boxed_slice();
        let got = try!(misc::io::read_fully(fs, &mut pr));
        if got < HEADER_SIZE_IN_BYTES {
            Err(Error::CorruptFile("invalid header"))
        } else {
            Ok(pr)
        }
    }

    fn parse(pr: &Box<[u8]>, cur: &mut usize) -> Result<(HeaderData, usize)> {
        fn readSegmentList(pr: &Box<[u8]>, cur: &mut usize) -> Result<(Vec<SegmentNum>, HashMap<SegmentNum, SegmentInfo>)> {
            let count = varint::read(&pr, cur) as usize;
            let mut a = Vec::with_capacity(count);
            let mut m = HashMap::with_capacity(count);
            for _ in 0 .. count {
                let g = varint::read(&pr, cur) as SegmentNum;
                a.push(g);
                let root_page = varint::read(&pr, cur) as PageNum;
                let blocks = BlockList::read(pr, cur);
                let level = varint::read(&pr, cur) as u32;
                if !blocks.contains_page(root_page) {
                    return Err(Error::RootPageNotInSegmentBlockList);
                }

                let location = SegmentLocation {
                    root_page: root_page,
                    blocks: blocks
                };
                let info = SegmentInfo {
                    location: location,
                    level: level,
                };
                m.insert(g,info);
            }
            Ok((a,m))
        }

        // --------

        let pgsz = misc::buf_advance::get_u32(&pr, cur) as usize;
        let changeCounter = varint::read(&pr, cur);
        let mergeCounter = varint::read(&pr, cur);
        let next_segnum = varint::read(&pr, cur);

        let (state, segments_info) = try!(readSegmentList(pr, cur));

        let hd = 
            HeaderData {
                current_state: state,
                segments_info: segments_info,
                changeCounter: changeCounter,
                mergeCounter: mergeCounter,
                next_segnum: next_segnum,
            };

        Ok((hd, pgsz))
    }

    fn calcNextPage(pgsz: usize, len: usize) -> PageNum {
        let numPagesSoFar = (if pgsz > len { 1 } else { len / pgsz }) as PageNum;
        numPagesSoFar + 1
    }

    // --------

    let mut fs = try!(OpenOptions::new()
            .read(true)
            .create(true)
            .open(&path));

    let len = try!(fs.metadata()).len();
    if len > 0 {
        try!(fs.seek(SeekFrom::Start(0 as u64)));
        let pr = try!(read(&mut fs));
        let mut cur = 0;
        let (h, pgsz) = try!(parse(&pr, &mut cur));
        let nextAvailablePage = calcNextPage(pgsz, len as usize);
        Ok((h, pgsz, nextAvailablePage))
    } else {
        // TODO shouldn't this use settings passed in?
        let defaultPageSize = DEFAULT_SETTINGS.DefaultPageSize;
        let h = {
            HeaderData {
                segments_info: HashMap::new(),
                current_state: Vec::new(),
                changeCounter: 0,
                mergeCounter: 0,
                next_segnum: 1,
            }
        };
        let nextAvailablePage = calcNextPage(defaultPageSize, HEADER_SIZE_IN_BYTES);
        Ok((h, defaultPageSize, nextAvailablePage))
    }

}

fn list_all_blocks(h: &HeaderData, pgsz: usize) -> BlockList {
    let mut blocks = BlockList::new();

    let headerBlock = PageBlock::new(1, (HEADER_SIZE_IN_BYTES / pgsz) as PageNum);
    blocks.add_block_no_reorder(headerBlock);

    for info in h.segments_info.values() {
        blocks.add_blocklist_no_reorder(&info.location.blocks);
    }

    blocks.sort_and_consolidate();

    blocks
}

use std::sync::Mutex;
use std::sync::RwLock;

#[derive(Debug)]
struct Space {
    nextPage: PageNum,

    freeBlocks: BlockList,

    nextCursorNum: u64,
    cursors: HashMap<u64, SegmentNum>,

    // a zombie segment is one that was replaced by a merge, but
    // when the merge was done, it could not be reclaimed as free
    // blocks because there was an open cursor on it.
    zombie_segments: HashMap<SegmentNum, SegmentInfo>,
}

pub struct PendingMerge {
    old_segments: Vec<SegmentNum>,
    merge_level: u32,
    new_segment: Option<SegmentInfo>,
}

struct SafeMergeStuff {
    // TODO can we design a way to not need the following?
    // it keeps track of which segments are being merged,
    // so we don't try to merge something that is already being merged.
    merging: HashSet<SegmentNum>,
}

struct SafePagePool {
    // we keep a pool of page buffers so we can lend them to
    // SegmentCursors.
    pages: Vec<Box<[u8]>>,
}

// there can be only one InnerPart instance per path
struct InnerPart {
    path: String,
    pgsz: usize,
    settings: DbSettings,

    // TODO are we concerned here about readers starving the
    // writers?  In other words, so many cursors that a merge
    // cannot get committed?
    header: RwLock<HeaderData>,

    space: Mutex<Space>,
    mergeStuff: Mutex<SafeMergeStuff>,
    pagepool: Mutex<SafePagePool>,
}

enum AutomergeMessage {
    NewSegment(SegmentNum, u32),
    Terminate,
}

pub struct WriteLock {
    inner: std::sync::Arc<InnerPart>,
    notify_automerge: Vec<mpsc::Sender<AutomergeMessage>>,
}

impl WriteLock {
    pub fn commit_segment(&self, new_seg: SegmentLocation) -> Result<()> {
        let segnum = try!(self.inner.commit_segment(new_seg));
        try!(self.notify_automerge[0].send(AutomergeMessage::NewSegment(segnum, 0)).map_err(wrap_err));
        Ok(())
    }

    pub fn commit_merge(&self, pm: PendingMerge) -> Result<()> {
        let notify =
            match pm.new_segment {
                Some(ref info) => {
                    if info.level == pm.merge_level {
                        // we only want to notify if this was a promotion.
                        // this merge was just cleaning up a level and
                        // leaving the resulting segment in that same level.
                        None
                    } else {
                        // this merge resulted in a segment getting promoted
                        // to the next level.  need to notify that level,
                        // because it might need a merge now.
                        Some(info.level)
                    }
                },
                None => {
                    // this merge did not result in a segment.  this happens
                    // because tombstones.
                    None
                },
            };
        let new_segnum = try!(self.inner.commit_merge(pm));
        match notify {
            Some(level) => {
                // the unwrap in the following line is correct.
                // if notify.is_some(), then commit_merge() has to
                // return a segment number.
                let new_segnum = new_segnum.unwrap();

                // nothing gets "promoted" to level 0.
                // new segments start in level 0
                assert!(level > 0);
                assert!((level as usize) < self.notify_automerge.len());
                try!(self.notify_automerge[level as usize].send(AutomergeMessage::NewSegment(new_segnum, level as u32)).map_err(wrap_err));
            },
            None => {
            },
        }
        Ok(())
    }
}

pub struct DatabaseFile {
    inner: std::sync::Arc<InnerPart>,

    // there can be only one of the following per path
    write_lock: Mutex<WriteLock>,
}

impl DatabaseFile {
    pub fn new(path: String, settings: DbSettings) -> Result<std::sync::Arc<DatabaseFile>> {

        // TODO we should pass in settings to read_header, right?
        let (header, pgsz, first_available_page) = try!(read_header(&path));

        // when we first open the file, we find all the blocks that are in use by
        // an active segment.  all OTHER blocks are considered free.
        let mut blocks = list_all_blocks(&header, pgsz);
        //println!("blocks in use: {:?}", blocks);
        let last_page_used = blocks.last_page();
        blocks.invert();
        if first_available_page > (last_page_used + 1) {
            let blk = PageBlock::new(last_page_used + 1, first_available_page - 1);
            blocks.add_block_no_reorder(blk);
            // TODO it is tempting to truncate the file here.  but this might not
            // be the right place.  we should preserve the ability to open a file
            // read-only.
        }

        // we want the largest blocks at the front of the list
        // two blocks of the same size?  sort earlier block first.
        blocks.sort_by_size_desc_page_asc();

        let space = Space {
            // TODO maybe we should just ignore the actual end of the file
            // and set nextPage to last_page_used + 1, and not add the block above
            nextPage: first_available_page, 
            freeBlocks: blocks,
            nextCursorNum: 1,
            cursors: HashMap::new(),
            zombie_segments: HashMap::new(),
        };

        let mergeStuff = SafeMergeStuff {
            merging: HashSet::new(),
        };

        let pagepool = SafePagePool {
            pages: vec![],
        };

        // each merge level is handled by its own thread.  a Rust channel is used to
        // notify that thread that there may be merge work to be done.

        let mut senders = vec![];
        let mut receivers = vec![];
        for _level in 0 .. LEVEL_SIZE_LIMITS_IN_KB.len() {
            let (tx, rx): (mpsc::Sender<AutomergeMessage>, mpsc::Receiver<AutomergeMessage>) = mpsc::channel();
            senders.push(tx);
            receivers.push(rx);
        }

        let inner = InnerPart {
            path: path,
            pgsz: pgsz,
            settings: settings, 
            header: RwLock::new(header),
            space: Mutex::new(space),
            mergeStuff: Mutex::new(mergeStuff),
            pagepool: Mutex::new(pagepool),
        };

        let inner = std::sync::Arc::new(inner);

        let lck = 
            WriteLock { 
                inner: inner.clone(),
                notify_automerge: senders,
            };

        let db = DatabaseFile {
            inner: inner,
            write_lock: Mutex::new(lck),
        };
        let db = std::sync::Arc::new(db);

        // TODO so when we do send Terminate messages to these threads?
        // impl Drop for DatabaseFile ? 

        for (closure_level, rx) in receivers.into_iter().enumerate() {
            let db = db.clone();
            thread::spawn(move || {
                loop {
                    match rx.recv() {
                        Ok(msg) => {
                            match msg {
                                AutomergeMessage::NewSegment(new_segnum, level) => {
                                    assert!(level == (closure_level as u32));
                                    match db.automerge_level(new_segnum, level) {
                                        Ok(()) => {
                                        },
                                        Err(e) => {
                                            // TODO what now?
                                            println!("{:?}", e);
                                            panic!();
                                        },
                                    }
                                },
                                AutomergeMessage::Terminate => {
                                    break;
                                },
                            }
                        },
                        Err(e) => {
                            // TODO what now?
                            println!("{:?}", e);
                            panic!();
                        },
                    }
                }
            });

        }

        Ok(db)
    }

    fn automerge_level(&self, new_segnum: SegmentNum, level: u32) -> Result<()> {
        assert!((level as usize) < LEVEL_SIZE_LIMITS_IN_KB.len());
        let promotion =
            if level == 0 {
                // all new segments come in at level 0.
                // we want to merge and promote them to level 1 as
                // quickly as possible.
                MergePromotionRule::Promote
            } else if (level as usize) == LEVEL_SIZE_LIMITS_IN_KB.len() - 1 {
                // nothing gets promoted out of the last level.
                MergePromotionRule::Stay
            } else {
                // for all the levels in between, we promote when the
                // level reaches a certain threshold of size.
                let mb = LEVEL_SIZE_LIMITS_IN_KB[level as usize];
                let bytes = mb * 1024;
                let pages = (bytes / (self.inner.pgsz as u64)) as PageCount;
                MergePromotionRule::Threshold(pages)
            };
        match try!(self.merge(level, 2, 8, promotion)) {
            Some(seg) => {
                let lck = try!(self.get_write_lock());
                try!(lck.commit_merge(seg));
            },
            None => {
            },
        }
        Ok(())
    }

    // TODO func to ask for the write lock without blocking?

    pub fn get_write_lock(&self) -> Result<std::sync::MutexGuard<WriteLock>> {
        let lck = try!(self.write_lock.lock());
        Ok(lck)
    }

    // the following methods are passthrus, exposing inner
    // stuff publicly.

    pub fn open_cursor(&self) -> Result<LivingCursor> {
        InnerPart::open_cursor(&self.inner)
    }

    pub fn open_cursor_on_page(&self, pg: PageNum) -> Result<PageCursor> {
        InnerPart::open_cursor_on_page(&self.inner, pg)
    }

    // TODO this might want to be just LeafPage
    pub fn open_cursor_on_leaf_page(&self, pg: PageNum) -> Result<LeafCursor> {
        InnerPart::open_cursor_on_leaf_page(&self.inner, pg)
    }

    pub fn open_cursor_on_parent_page(&self, pg: PageNum) -> Result<ParentCursor> {
        InnerPart::open_cursor_on_parent_page(&self.inner, pg)
    }

    pub fn read_parent_page(&self, pg: PageNum) -> Result<ParentPage> {
        InnerPart::read_parent_page(&self.inner, pg)
    }

    pub fn write_segment(&self, pairs: BTreeMap<Box<[u8]>, Blob>) -> Result<SegmentLocation> {
        InnerPart::write_segment(&self.inner, pairs)
    }

    pub fn merge(&self, level: u32, min_segs: usize, max_segs: usize, promote: MergePromotionRule) -> Result<Option<PendingMerge>> {
        InnerPart::merge(&self.inner, level, min_segs, max_segs, promote)
    }

    pub fn list_segments(&self) -> Result<(Vec<SegmentNum>, HashMap<SegmentNum, SegmentInfo>)> {
        InnerPart::list_segments(&self.inner)
    }

    pub fn release_pending_segment(&self, location: SegmentLocation) -> Result<()> {
        InnerPart::release_pending_segment(&self.inner, location)
    }

    pub fn list_free_blocks(&self) -> Result<BlockList> {
        InnerPart::list_free_blocks(&self.inner)
    }

    pub fn get_page(&self, pgnum: PageNum) -> Result<Box<[u8]>> {
        InnerPart::get_page(&self.inner, pgnum)
    }
}

// TODO this could be generic
fn slice_within(sub: &[SegmentNum], within: &[SegmentNum]) -> Result<usize> {
    match within.iter().position(|&g| g == sub[0]) {
        Some(ndx_first) => {
            let count = sub.len();
            if sub == &within[ndx_first .. ndx_first + count] {
                Ok(ndx_first)
            } else {
                Err(Error::Misc(String::from("not contiguous")))
            }
        },
        None => {
            Err(Error::Misc(String::from("not contiguous")))
        },
    }
}

impl InnerPart {

    fn return_page_to_pool(&self, page: Box<[u8]>) {
        //println!("return_page_to_pool");
        match self.pagepool.try_lock() {
            Ok(mut pagepool) => {
                //println!("returned a page");
                // TODO arbitrary hard-coded limit
                if pagepool.pages.len() < 100 {
                    pagepool.pages.push(page);
                }
            },
            Err(std::sync::TryLockError::Poisoned(_)) => {
                panic!("poisoned");
            },
            Err(std::sync::TryLockError::WouldBlock) => {
                //println!("could NOT return a page to the pool");
            },
        }
    }

    fn cursor_dropped(&self, segnum: SegmentNum, csrnum: u64) {
        //println!("cursor_dropped");
        let mut space = self.space.lock().unwrap(); // gotta succeed
        let seg = space.cursors.remove(&csrnum).expect("gotta be there");
        assert_eq!(seg, segnum);
        // TODO hey.  actually we can't reclaim this zombie unless we know
        // that the cursor we just dropped was the only cursor on the
        // zombie segment.
        match space.zombie_segments.remove(&segnum) {
            Some(info) => {
                Self::addFreeBlocks(&mut space, &self.path, self.pgsz, info.location.blocks.blocks);
            },
            None => {
            },
        }
    }

    fn getBlock(&self, space: &mut Space, req: BlockRequest) -> PageBlock {
        fn find_block_starting_at(space: &mut Space, start: Vec<PageNum>) -> Option<usize> {
// TODO which way should we nest these two loops?
            for i in 0 .. space.freeBlocks.len() {
                for j in 0 .. start.len() {
                    if space.freeBlocks[i].firstPage == start[j] {
                        return Some(i);
                    }
                }
            }
            None
        }

        fn find_block_minimum_size(space: &mut Space, size: PageCount) -> Option<usize> {
            // space.freeBlocks is sorted by size desc, so we only
            // need to check the first block.
            if space.freeBlocks[0].count_pages() >= size {
                return Some(0);
            } else {
                // if this block isn't big enough, none of the ones after it will be
                return None;
            }
        }

        fn find_block_after_minimum_size(space: &mut Space, after: PageNum, size: PageCount) -> Option<usize> {
            for i in 0 .. space.freeBlocks.len() {
                if space.freeBlocks[i].count_pages() >= size {
                    if space.freeBlocks[i].firstPage > after {
                        return Some(i);
                    } else {
                        // big enough, but not "after".  keep looking.
                    }
                } else {
                    // if this block isn't big enough, none of the ones after it will be
                    return None;
                }
            }
            None
        }

        enum FromWhere {
            End(PageCount),
            FirstFree,
            SpecificFree(usize),
        }

        if let Some(after) = req.get_after() {
            assert!(space.nextPage > after);
        }

        let from =
            if space.freeBlocks.is_empty() {
                FromWhere::End(self.settings.PagesPerBlock)
            } else if req.start_is(space.nextPage) {
                FromWhere::End(self.settings.PagesPerBlock)
            } else {
                match req {
                    BlockRequest::Any => {
                        FromWhere::FirstFree
                    },
                    BlockRequest::StartOrAfterMinimumSize(start, after, size) => {
                        assert!(start.iter().all(|start| *start > after));
                        if let Some(i) = find_block_starting_at(space, start) {
                            FromWhere::SpecificFree(i)
                        } else if let Some(i) = find_block_after_minimum_size(space, after, size) {
                            FromWhere::SpecificFree(i)
                        } else {
                            FromWhere::End(std::cmp::max(size, self.settings.PagesPerBlock))
                        }
                    },
                    BlockRequest::MinimumSize(size) => {
                        if let Some(i) = find_block_minimum_size(space, size) {
                            FromWhere::SpecificFree(i)
                        } else {
                            FromWhere::End(std::cmp::max(size, self.settings.PagesPerBlock))
                        }
                    },
                    BlockRequest::StartOrAny(start) => {
                        if let Some(i) = find_block_starting_at(space, start) {
                            FromWhere::SpecificFree(i)
                        } else {
                            FromWhere::FirstFree
                        }
                    },

                }
            };

        match from {
            FromWhere::FirstFree => {
                space.freeBlocks.remove_block(0)
            },
            FromWhere::SpecificFree(i) => {
                space.freeBlocks.remove_block(i)
            },
            FromWhere::End(size) => {
                let newBlk = PageBlock::new(space.nextPage, space.nextPage + size - 1) ;
                space.nextPage = space.nextPage + size;
                newBlk
            },
        }
    }

    fn OpenForWriting(&self) -> io::Result<File> {
        OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
    }

    fn OpenForReading(&self) -> io::Result<File> {
        OpenOptions::new()
                .read(true)
                .open(&self.path)
    }

    fn open_file_for_cursor(&self) -> io::Result<std::rc::Rc<std::cell::RefCell<File>>> {
        let f = try!(self.OpenForReading());
        let f = std::cell::RefCell::new(f);
        let f = std::rc::Rc::new(f);
        Ok(f)
    }

    // this code should not be called in a release build.  it helps
    // finds problems by zeroing out pages in blocks that
    // have been freed.
    #[cfg(remove_me)]
    fn stomp(&self, blocks:Vec<PageBlock>) -> Result<()> {
        let bad = vec![0;self.pgsz as usize].into_boxed_slice();
        let mut fs = try!(OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path));
        for b in blocks {
            for x in b.firstPage .. b.lastPage+1 {
                try!(utils::SeekPage(&mut fs, self.pgsz, x));
                try!(fs.write(&bad));
            }
        }
        Ok(())
    }

    // TODO should this accept a BlockList instead of a Vec<>
    fn addFreeBlocks(space: &mut Space, path: &str, page_size: usize, blocks: Vec<PageBlock>) -> Result<()> {

        // all additions to the freeBlocks list should happen here
        // by calling this function.
        //
        // the list is kept consolidated and sorted by size descending.
        // unfortunately this requires two sorts, and they happen here
        // inside a critical section.  but the benefit is considered
        // worth the trouble.
        
        // TODO it is important that freeBlocks contains no overlaps.
        // add debug-only checks to verify?

        //println!("adding free blocks: {:?}", blocks);
        for b in blocks {
            space.freeBlocks.add_block_no_reorder(b);
        }
        space.freeBlocks.sort_and_consolidate();
        if space.freeBlocks.len() > 0 && space.freeBlocks.last_page() == space.nextPage - 1 {
            let i_last_block = space.freeBlocks.len() - 1;
            let blk = space.freeBlocks.remove_block(i_last_block);
            //println!("    killing free_at_end: {:?}", blk);
            space.nextPage = blk.firstPage;
        }

        let file_length = try!(std::fs::metadata(path)).len();
        let page_size = page_size as u64;
        let first_page_beyond = (file_length / page_size + 1) as u32;
        if first_page_beyond > space.nextPage {
            let fs = try!(OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    );
            try!(fs.set_len(((space.nextPage - 1) as u64) * page_size));
        }

        // we want the largest blocks at the front of the list
        // two blocks of the same size?  sort earlier block first.
        space.freeBlocks.sort_by_size_desc_page_asc();

        //println!("    space now: {:?}", space);
        Ok(())
    }

    // a stored segmentinfo for a segment is a single blob of bytes.
    // root page
    // number of pairs
    // each pair is startBlock,countBlocks
    // level
    // all in varints

    fn write_header(&self, 
                   dest: &mut HeaderData, 
                   fs: &mut File, 
                   hdr: HeaderData
                  ) -> Result<()> {

        fn build_segment_list(h: &HeaderData) -> Vec<u8> {
            let mut pb = vec![];
            misc::push_varint(&mut pb, h.current_state.len() as u64);
            for g in h.current_state.iter() {
                misc::push_varint(&mut pb, *g);
                match h.segments_info.get(&g) {
                    Some(info) => {
                        misc::push_varint(&mut pb, info.location.root_page as u64);
                        info.location.blocks.encode(&mut pb);
                        misc::push_varint(&mut pb, info.level as u64);
                    },
                    None => panic!("segment num in current_state but not in segments_info")
                }
            }

            pb
        }

        let mut pb = PageBuilder::new(HEADER_SIZE_IN_BYTES);
        // TODO format version number
        pb.PutInt32(self.pgsz as u32);

        pb.PutVarint(hdr.changeCounter);
        pb.PutVarint(hdr.mergeCounter);
        pb.PutVarint(hdr.next_segnum);

        let pbSegList = build_segment_list(&hdr);

        // TODO the + 1 in the following line is probably no longer needed.
        // I think it used to be the flag indicating header overflow.
        if pb.Available() >= (pbSegList.len() + 1) {
            pb.PutArray(&pbSegList);
        } else {
            return Err(Error::Misc(String::from("header too big")));
        }

        try!(fs.seek(SeekFrom::Start(0)));
        try!(pb.Write(fs));
        try!(fs.flush());
        *dest = hdr;
        Ok(())
    }

    fn get_cursor_on_active_segment(
        inner: &std::sync::Arc<InnerPart>, 
        header: &HeaderData,
        g: SegmentNum,
        f: std::rc::Rc<std::cell::RefCell<File>>,
        ) -> Result<Lend<PageCursor>> {

        match header.segments_info.get(&g) {
            None => Err(Error::Misc(String::from("get_cursor_on_active_segment: segment not found"))),
            Some(seg) => {
                let mut space = try!(inner.space.lock());
                let csrnum = space.nextCursorNum;
                let foo = inner.clone();
                let done = move |_| -> () {
                    // TODO this wants to propagate errors
                    foo.cursor_dropped(g, csrnum);
                };
                let buf = try!(Self::get_loaner_page(inner));
                let csr = try!(PageCursor::new(&inner.path, f, seg.location.root_page, buf));
                let csr = Lend::new(csr, box done);

                space.nextCursorNum = space.nextCursorNum + 1;
                let was = space.cursors.insert(csrnum, g);
                assert!(was.is_none());
                Ok(csr)
            }
        }
    }

    fn get_loaner_page(inner: &std::sync::Arc<InnerPart>) -> Result<Lend<Box<[u8]>>> {
        let page = {
            match inner.pagepool.try_lock() {
                Ok(mut pagepool) => {
                    match pagepool.pages.pop() {
                        Some(p) => p,
                        None => vec![0; inner.pgsz].into_boxed_slice(),
                    }
                },
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(Error::Poisoned);
                },
                Err(std::sync::TryLockError::WouldBlock) => {
                    vec![0; inner.pgsz].into_boxed_slice()
                },
            }
        };
        let foo = inner.clone();
        let done_page = move |p| -> () {
            foo.return_page_to_pool(p);
        };
        let page = Lend::new(page, box done_page);
        Ok(page)
    }

    fn open_cursor_on_page(inner: &std::sync::Arc<InnerPart>, pg: PageNum) -> Result<PageCursor> {
        let mut buf = try!(Self::get_loaner_page(inner));
        let f = try!(inner.open_file_for_cursor());
        let cursor = try!(PageCursor::new(&inner.path, f, pg, buf));
        Ok(cursor)
    }

    fn open_cursor_on_leaf_page(inner: &std::sync::Arc<InnerPart>, pg: PageNum) -> Result<LeafCursor> {
        let mut buf = try!(Self::get_loaner_page(inner));
        let f = try!(inner.open_file_for_cursor());

        {
            let f = &mut *(f.borrow_mut());
            try!(utils::SeekPage(f, buf.len(), pg));
            try!(misc::io::read_fully(f, &mut buf));
        }

        let page = try!(LeafPage::new_already_read_page(&inner.path, f, pg, buf));
        let cursor = LeafCursor::new(page);
        Ok(cursor)
    }

    fn open_cursor_on_parent_page(inner: &std::sync::Arc<InnerPart>, pg: PageNum) -> Result<ParentCursor> {
        let mut buf = try!(Self::get_loaner_page(inner));
        let f = try!(inner.open_file_for_cursor());

        {
            let f = &mut *(f.borrow_mut());
            try!(utils::SeekPage(f, buf.len(), pg));
            try!(misc::io::read_fully(f, &mut buf));
        }

        let page = try!(ParentPage::new_already_read_page(&inner.path, f, pg, buf));
        let cursor = try!(ParentCursor::new(page));
        Ok(cursor)
    }

    fn read_parent_page(inner: &std::sync::Arc<InnerPart>, pg: PageNum) -> Result<ParentPage> {
        let mut buf = try!(Self::get_loaner_page(inner));
        let f = try!(inner.open_file_for_cursor());

        {
            let f = &mut *(f.borrow_mut());
            try!(utils::SeekPage(f, buf.len(), pg));
            try!(misc::io::read_fully(f, &mut buf));
        }

        let page = try!(ParentPage::new_already_read_page(&inner.path, f, pg, buf));
        Ok(page)
    }

    fn open_cursor(inner: &std::sync::Arc<InnerPart>) -> Result<LivingCursor> {
        let header = try!(inner.header.read());
        let f = try!(inner.open_file_for_cursor());
        let cursors = 
            header.current_state
            .iter()
            .map(|g| Self::get_cursor_on_active_segment(inner, &header, *g, f.clone()))
            .collect::<Result<Vec<_>>>();
        let cursors = try!(cursors);
        let mc = MultiCursor::Create(cursors);
        let lc = LivingCursor::Create(mc);
        Ok(lc)
    }

    fn get_page(inner: &std::sync::Arc<InnerPart>, pgnum: PageNum) -> Result<Box<[u8]>> {
        // OpenForReading?
        let mut f = try!(OpenOptions::new()
                .read(true)
                .create(true)
                .open(&inner.path));
        let mut buf = vec![0; inner.pgsz].into_boxed_slice();
        try!(utils::SeekPage(&mut f, inner.pgsz, pgnum));
        try!(misc::io::read_fully(&mut f, &mut buf));
        Ok(buf)
    }

    fn list_free_blocks(inner: &std::sync::Arc<InnerPart>) -> Result<BlockList> {
        let space = try!(inner.space.lock());
        Ok(space.freeBlocks.clone())
    }

    fn release_pending_segment(inner: &std::sync::Arc<InnerPart>, location: SegmentLocation) -> Result<()> {
        let mut space = try!(inner.space.lock());
        try!(Self::addFreeBlocks(&mut space, &inner.path, inner.pgsz, location.blocks.blocks));
        Ok(())
    }

    fn list_segments(inner: &std::sync::Arc<InnerPart>) -> Result<(Vec<SegmentNum>, HashMap<SegmentNum, SegmentInfo>)> {
        let header = try!(inner.header.read());
        let a = header.current_state.clone();
        let b = header.segments_info.clone();
        Ok((a,b))
    }

    fn commit_segment(&self, new_seg: SegmentLocation) -> Result<SegmentNum> {
        // all new segments are given level 0
        let new_seg = SegmentInfo {
            location: new_seg,
            level: 0,
        };
        let mut header = try!(self.header.write());

        // TODO assert new_seg shares no pages with any seg in current state

        let mut newHeader = header.clone();

        let new_segnum = newHeader.next_segnum;
        newHeader.next_segnum += 1;
        newHeader.segments_info.insert(new_segnum, new_seg);
        newHeader.current_state.insert(0, new_segnum);

        newHeader.changeCounter = newHeader.changeCounter + 1;

        let mut fs = try!(self.OpenForWriting());
        try!(self.write_header(&mut header, &mut fs, newHeader));

        // note that we intentionally do not release the writeLock here.
        // you can change the segment list more than once while holding
        // the writeLock.  the writeLock gets released when you Dispose() it.

        Ok(new_segnum)
    }

    fn write_segment(inner: &std::sync::Arc<InnerPart>, pairs: BTreeMap<Box<[u8]>, Blob>) -> Result<SegmentLocation> {
        let source = pairs.into_iter().map(|t| {
            let (k, v) = t;
            Ok(kvp {Key:k, Value:v})
        });
        let pw = try!(PageWriter::new(inner.clone()));
        let seg = try!(create_segment(pw, source));
        Ok(seg)
    }

    fn write_merge_segment(inner: &std::sync::Arc<InnerPart>, cursor: Box<ICursor>) -> Result<SegmentLocation> {
        let source = CursorIterator::new(cursor);
        let pw = try!(PageWriter::new(inner.clone()));
        let seg = try!(create_segment(pw, source));
        Ok(seg)
    }

    fn merge(inner: &std::sync::Arc<InnerPart>, merge_level: u32, min_segs: usize, max_segs: usize, promote: MergePromotionRule) -> Result<Option<PendingMerge>> {
        assert!(min_segs <= max_segs);
        let step1 = {
            let header = try!(inner.header.read());

            if header.current_state.len() == 0 {
                return Ok(None)
            }

            //println!("merge_level: {}", merge_level);
            //println!("promote: {:?}", promote);
            //println!("current_state: {:?}", header.current_state);
            //println!("segments_info: {:?}", header.segments_info);
            let mut level_sizes = HashMap::new();
            for (_, ref info) in header.segments_info.iter() {
                let pages = info.location.count_pages();
                match level_sizes.entry(&info.level) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        let n = e.get_mut();
                        *n = *n + pages;
                    },
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(pages);
                    },
                };
            }
            //println!("level_sizes: {:?}", level_sizes);

            let level_group = 
                header.current_state
                .iter()
                .filter(|g| {
                    let info = header.segments_info.get(&g).unwrap();
                    info.level == merge_level
                })
                .map(|g| *g)
                .collect::<Vec<SegmentNum>>();

            //println!("level_group: {:?}", level_group);
            //println!("segment in level: {}", level_group.len());

            if level_group.len() == 0 {
                //println!("no merge");
                return Ok(None)
            }

            let pages_in_merge_level = level_sizes[&merge_level];
            //println!("pages_in_merge_level: {:?}", pages_in_merge_level);

            // make sure this is contiguous
            assert!(slice_within(level_group.as_slice(), header.current_state.as_slice()).is_ok());

            let mut merge_seg_nums = Vec::new();

            let mut mergeStuff = try!(inner.mergeStuff.lock());

            // we can merge any contiguous set of not-already-being-merged 
            // segments at the end of the group.  if we merge something
            // that is not at the end of the group, we could end up with
            // level groups not being contiguous.  TODO techically, the
            // previous statement is true only if we are promoting the level.

            for g in level_group.iter().rev() {
                if mergeStuff.merging.contains(g) {
                    break;
                } else {
                    merge_seg_nums.push(*g);
                }
            }

            if merge_seg_nums.len() >= min_segs {
                merge_seg_nums.truncate(max_segs);

                // right now the merge_seg_nums list is in reverse order because we searched with a
                // reverse iterator just above.  reverse it again to make it right.
                merge_seg_nums.reverse();

                //println!("segments being merged: {:?}", merge_seg_nums);
                let pages_in_merge_segments: PageCount = 
                    merge_seg_nums
                    .iter()
                    .map(
                        |g| {
                            let info = header.segments_info.get(g).unwrap();
                            info.location.count_pages()
                        })
                    .sum();
                //println!("pages_in_merge_segments: {}", pages_in_merge_segments);
                assert!(pages_in_merge_segments <= pages_in_merge_level);
                let f = try!(inner.open_file_for_cursor());
                let cursors = 
                    merge_seg_nums
                    .iter()
                    .map(|g| Self::get_cursor_on_active_segment(inner, &header, *g, f.clone()))
                    .collect::<Result<Vec<_>>>();
                let cursors = try!(cursors);

                for g in merge_seg_nums.iter() {
                    mergeStuff.merging.insert(*g);
                }

                let cursor = {
                    let mc = MultiCursor::Create(cursors);
                    mc
                };

                let last_seg_being_merged = merge_seg_nums[merge_seg_nums.len() - 1];
                let pos_last_seg = header.current_state.iter().position(|s| *s == last_seg_being_merged).expect("gotta be there");
                let count_segments_behind = header.current_state.len() - (pos_last_seg + 1);

                let cursor: Box<ICursor> =
                    if count_segments_behind == 0 {
                        // we are merging the last segments in the current state.
                        // there is nothing behind.
                        // so all tombstones can be filtered.
                        // so we just wrap in a LivingCursor.
                        let cursor = LivingCursor::Create(cursor);
                        box cursor
                    } else if count_segments_behind <= 8 {
                        // TODO arbitrary hard-coded limit in the line above

                        // there are segments behind the ones we are merging.
                        // we can only filter a tombstone if its key is not present behind.

                        // TODO getting all these cursors can be really expensive.

                        // TODO if we knew there were no tombstones in the segments to be merged,
                        // we would not bother with this.

                        // TODO if there are a LOT of segments behind, this will fail because
                        // of opening too many files.

                        // TODO we need a way to cache these cursors.  maybe these "behind"
                        // cursors are special, and are stored here in the header somewhere.
                        // the next time we do a merge, we can reuse them.  they need to get
                        // cleaned up at some point.

                        // TODO capacity
                        let mut behind = vec![];
                        for s in &header.current_state[pos_last_seg + 1 ..] {
                            let s = *s;
                            let cursor = try!(Self::get_cursor_on_active_segment(inner, &header, s, f.clone()));
                            behind.push(cursor);
                        }
                        // TODO to allow reuse of these behind cursors, we should pass
                        // them as references, don't transfer ownership.  but then they
                        // will need to be owned somewhere else.
                        let cursor = FilterTombstonesCursor::new(cursor, behind);
                        box cursor
                    } else {
                        box cursor
                    };

                Some((merge_seg_nums, pages_in_merge_segments, pages_in_merge_level, cursor))
            } else {
                //println!("no merge");
                None
            }
        };

        match step1 {
            Some((merge_seg_nums, pages_in_merge_segments, pages_in_merge_level, mut cursor)) => {
                // TODO if something goes wrong here, the function will exit with
                // an error but mergeStuff.merging will still contain the segments we are
                // trying to merge, which will prevent them from EVER being merged.

                // note that cursor.First() should NOT have already been called
                try!(cursor.First());
                let seg = 
                    if cursor.IsValid() {
                        let location = try!(Self::write_merge_segment(inner, cursor));
                        let pages_in_new_segment = location.count_pages();
                        //println!("pages_in_new_segment: {}", pages_in_new_segment);
                        // TODO is the following assert always true?
                        // TODO assert!(pages_in_new_segment <= pages_in_merge_segments);
                        let level =
                            match promote {
                                MergePromotionRule::Promote => {
                                    merge_level + 1
                                },
                                MergePromotionRule::Stay => {
                                    merge_level
                                },
                                MergePromotionRule::Threshold(n) => {
                                    if pages_in_merge_level >= n {
                                        merge_level + 1
                                    } else {
                                        merge_level
                                    }
                                },
                            };
                        let seg =
                            SegmentInfo {
                                location: location,
                                level: level,
                            };
                        Some(seg)
                    } else {
                        None
                    };
                let pm = 
                    PendingMerge {
                        old_segments: merge_seg_nums,
                        new_segment: seg,
                        merge_level: merge_level,
                    };
                Ok(Some(pm))
            },
            None => {
                Ok(None)
            },
        }
    }

    fn commit_merge(&self, pm: PendingMerge) -> Result<Option<SegmentNum>> {
        let (segmentsBeingReplaced, new_segnum) = {
            let mut header = try!(self.header.write());

            // TODO assert new_seg shares no pages with any seg in current state

            // we need the list of segments which were merged.  we make a copy of
            // so that we're not keeping a reference that inhibits our ability to
            // get other references a little later in the function.

            // TODO why does this Set need to be created?
            let oldAsSet: HashSet<SegmentNum> = pm.old_segments.iter().map(|g| *g).collect();
            assert!(oldAsSet.len() == pm.old_segments.len());

            // now we need to verify that the segments being replaced are in current_state
            // and contiguous.

            let ndxFirstOld = try!(slice_within(pm.old_segments.as_slice(), header.current_state.as_slice()));

            // now we construct a newHeader

            let mut newHeader = header.clone();

            // remove the old segmentinfos, keeping them for later

            let mut segmentsBeingReplaced = HashMap::with_capacity(oldAsSet.len());
            for g in &oldAsSet {
                let info = newHeader.segments_info.remove(g).expect("old seg not found in header.segments_info");
                segmentsBeingReplaced.insert(*g, info);
            }

            // remove old segments from current state

            for _ in &pm.old_segments {
                newHeader.current_state.remove(ndxFirstOld);
            }

            let new_segnum =
                match pm.new_segment {
                    None => {
                        None
                        // a merge resulted in what would have been an empty segment.
                        // this happens because tombstones
                    },
                    Some(new_seg) => {
                        let new_segnum = newHeader.next_segnum;
                        newHeader.next_segnum += 1;
                        newHeader.current_state.insert(ndxFirstOld, new_segnum);
                        newHeader.segments_info.insert(new_segnum, new_seg);
                        Some(new_segnum)
                    },
                };

            newHeader.mergeCounter = newHeader.mergeCounter + 1;

            let mut fs = try!(self.OpenForWriting());
            try!(self.write_header(&mut header, &mut fs, newHeader));

            (segmentsBeingReplaced, new_segnum)
        };

        {
            let mut mergeStuff = try!(self.mergeStuff.lock());
            for g in pm.old_segments {
                mergeStuff.merging.remove(&g);
            }
        }

        let mut segmentsToBeFreed = segmentsBeingReplaced;
        {
            let mut space = try!(self.space.lock());
            let segmentsWithACursor: HashSet<SegmentNum> = space.cursors.iter().map(|t| {let (_,segnum) = t; *segnum}).collect();
            for g in segmentsWithACursor {
                // don't free any segment that has a cursor.  yet.
                match segmentsToBeFreed.remove(&g) {
                    Some(z) => {
                        //println!("zombie: {:?}", z);
                        space.zombie_segments.insert(g, z);
                    },
                    None => {
                    },
                }
            }
            let mut blocksToBeFreed = Vec::new();
            for info in segmentsToBeFreed.values() {
                blocksToBeFreed.push_all(&info.location.blocks.blocks);
            }
            try!(Self::addFreeBlocks(&mut space, &self.path, self.pgsz, blocksToBeFreed));
        }

        // note that we intentionally do not release the writeLock here.
        // you can change the segment list more than once while holding
        // the writeLock.  the writeLock gets released when you Dispose() it.
        Ok(new_segnum)
    }

}

struct PageWriter {
    inner: std::sync::Arc<InnerPart>,
    f: File,
    list: BlockList,

    // TODO the following two could be BlockLists if we didn't have to worry about it reordering the list
    blocks: Vec<PageBlock>,
    group_blocks: Vec<PageBlock>,

    last_page: PageNum,
}

impl PageWriter {
    fn new(inner: std::sync::Arc<InnerPart>) -> Result<Self> {
        let f = try!(inner.OpenForWriting());
        let pw = PageWriter {
            inner: inner,
            f: f,
            list: BlockList::new(),
            blocks: vec![],
            group_blocks: vec![],
            last_page: 0,
        };
        Ok(pw)
    }

    fn request_block(&self, req: BlockRequest) -> Result<PageBlock> {
        let mut space = try!(self.inner.space.lock());
        let blk = self.inner.getBlock(&mut space, req);
        Ok(blk)
    }

    fn ensure_inventory(&mut self) -> Result<()> {
        assert!(self.blocks.len() <= 2);
        if self.blocks.is_empty() {
            let blk = try!(self.request_block(BlockRequest::MinimumSize(2)));
            self.blocks.push(blk);
            Ok(())
        } else if self.blocks.len() == 1 {
            if self.blocks[0].count_pages() == 1 {
                let want = self.blocks[0].lastPage + 1;
                let blk2 = try!(self.request_block(BlockRequest::StartOrAny(vec![want])));
                if blk2.firstPage == want {
                    self.blocks[0].lastPage = blk2.lastPage;
                } else {
                    self.blocks.push(blk2);
                }
            }
            Ok(())
        } else if self.blocks.len() == 2 {
            Ok(())
        } else {
            unreachable!();
        }
    }

    fn get_page_from(blocks: &mut Vec<PageBlock>) -> Result<PageNum> {
        assert!(!blocks.is_empty());
        if blocks[0].count_pages() == 1 {
            assert!(blocks.len() > 1);
            let blk = blocks.remove(0);
            assert!(blk.firstPage == blk.lastPage);
            Ok(blk.firstPage)
        } else {
            let pg = blocks[0].firstPage;
            blocks[0].firstPage += 1;
            Ok(pg)
        }
    }

    fn get_page(&mut self) -> Result<PageNum> {
        try!(self.ensure_inventory());
        let pg = try!(Self::get_page_from(&mut self.blocks));
        self.add_page_to_list(pg);
        Ok(pg)
    }

    fn ensure_group_inventory(&mut self, group: &BlockList, limit: usize) -> Result<()> {
        // TODO the limit parameter is the hard limit on the size of the
        // encoded blocklist for the group.  We can use this information,
        // in cases where we are getting close to the limit, to get more
        // desperate and request a large block at the end of the file.

        // TODO not sure what minimum size we should request.
        // we want overflows to be contiguous, but we also want to
        // avoid putting them at the end of the file.
        const MINIMUM_SIZE_FIRST_BLOCK_IN_GROUP: PageCount = 16;

        assert!(self.group_blocks.len() <= 2);
        if self.group_blocks.is_empty() {
            // if there is no inventory, then the group has gotta be empty,
            // because we don't allow the inventory to be completely depleted
            // during the processing of a group.
            assert!(group.is_empty());

            // TODO we might want to prefer a very early block, since all the other blocks
            // in this group are going to have to be after it.  

            // note that the free block list 
            // is secondary sorted by firstPage ASC.

            // suppose the min size is 10.  
            
            // Would we prefer a 500 page block late in the file?  making it less likely
            // that we will need another block at all?  but if we do need another block,
            // we will have ruled out all the free blocks before it.

            // or would we prefer a 10 page block early in the file?  this makes it more
            // likely we will need another block, but more of the free block list is
            // available for choosing.

            let blk = try!(self.request_block(BlockRequest::MinimumSize(MINIMUM_SIZE_FIRST_BLOCK_IN_GROUP)));
            self.group_blocks.push(blk);
            Ok(())
        } else if self.group_blocks.len() == 1 {
            if !group.is_empty() {
                assert!(self.group_blocks[0].firstPage > group.blocks[0].firstPage);
            }
            if self.group_blocks[0].count_pages() == 1 {
                // we always prefer a block which extends the one we've got in inventory
                let want = self.group_blocks[0].lastPage + 1;

                let req =
                    match group.len() {
                        0 => {
                            // group hasn't started yet.  so it doesn't care where the block is,
                            // but it soon will, because it will be given the currently available
                            // page, so we need to make sure we get something after that one.
                            let after = self.group_blocks[0].firstPage;

                            BlockRequest::StartOrAfterMinimumSize(vec![want], after, MINIMUM_SIZE_FIRST_BLOCK_IN_GROUP)
                        },
                        _ => {
                            // the one available page must fit on one of the blocks already
                            // in the group
                            assert!(group.would_extend_an_existing_block(self.group_blocks[0].firstPage));

                            // we would also prefer any block which would extend any of the
                            // blocks already in the group

                            let mut wants = Vec::with_capacity(4);
                            for i in 0 .. group.len() {
                                let pg = group.blocks[i].lastPage + 1;
                                if want != pg {
                                    wants.push(pg);
                                }
                            }
                            wants.insert(0, want);

                            // the group is running, so we cannnot accept any block before the
                            // first page of the group.
                            let after = group.blocks[0].firstPage;

                            // TODO use the limit provided for close cases
                            // TODO tune the numbers below
                            // TODO maybe the request size should be a formula instead of match cases

                            match group.len() {
                                0 => {
                                    // already handled above
                                    unreachable!();
                                },
                                1 => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 8)
                                },
                                2 => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 32)
                                },
                                3 => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 128)
                                },
                                4 => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 256)
                                },
                                5 => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 256)
                                },
                                _ => {
                                    BlockRequest::StartOrAfterMinimumSize(wants, after, 1024)
                                },
                            }
                        },
                    };
                let blk2 = try!(self.request_block(req));
                if !group.is_empty() {
                    assert!(blk2.firstPage > group.blocks[0].firstPage);
                }
                if blk2.firstPage == self.group_blocks[0].lastPage + 1 {
                    self.group_blocks[0].lastPage = blk2.lastPage;
                } else {
                    self.group_blocks.push(blk2);
                }
            }
            Ok(())
        } else if self.group_blocks.len() == 2 {
            if !group.is_empty() {
                assert!(self.group_blocks[0].firstPage > group.blocks[0].firstPage);
            }
            Ok(())
        } else {
            unreachable!();
        }
    }

    fn get_group_page(&mut self, group: &mut BlockList, limit: usize) -> Result<PageNum> {
        try!(self.ensure_group_inventory(group, limit));
        let pg = try!(Self::get_page_from(&mut self.group_blocks));
        self.add_page_to_list(pg);
        if group.blocks.len() > 0 {
            let first = group.blocks[0].firstPage;
            assert!(pg > first);
        }
        let extended = group.add_page_no_reorder(pg);
        // TODO only consol if above false?
        group.sort_and_consolidate();
        assert!(group.encoded_len() < limit);
        Ok(pg)
    }

    fn page_size(&self) -> usize {
        self.inner.pgsz
    }

    fn is_group_on_block_boundary(&mut self, group: &BlockList, limit: usize) -> Result<Option<PageNum>> {
        try!(self.ensure_group_inventory(group, limit));
        // at this point, the caller must be able to request 
        // two pages without change in the group inventory
        // don't reorder the inventory.
        // make sure we always take from the first block in inventory.

        if self.group_blocks[0].count_pages() > 1 {
            Ok(None)
        } else {
            assert!(self.group_blocks[0].count_pages() == 1);
            assert!(self.group_blocks.len() > 1);
            assert!(self.group_blocks[0].lastPage + 1 != self.group_blocks[1].firstPage);
            Ok(Some(self.group_blocks[1].firstPage))
        }
    }

    fn add_page_to_list(&mut self, pg: PageNum) {
        let extended = self.list.add_page_no_reorder(pg);
        // TODO only consol if above false?
        self.list.sort_and_consolidate();
    }

    fn do_write(&mut self, buf: &[u8], pg: PageNum) -> Result<()> {
        if pg != self.last_page + 1 {
            try!(utils::SeekPage(&mut self.f, self.inner.pgsz, pg));
        }
        try!(self.f.write_all(buf));
        self.last_page = pg;
        Ok(())
    }

    fn write_group_page(&mut self, buf: &[u8], group: &mut BlockList, limit: usize) -> Result<()> {
        let pg = try!(self.get_group_page(group, limit));
        try!(self.do_write(buf, pg));
        Ok(())
    }

    fn write_page(&mut self, buf: &[u8]) -> Result<PageNum> {
        let pg = try!(self.get_page());
        try!(self.do_write(buf, pg));
        Ok(pg)
    }

    fn end(self) -> Result<BlockList> {
        if !self.blocks.is_empty() || !self.group_blocks.is_empty() {
            let mut space = try!(self.inner.space.lock());
            if !self.blocks.is_empty() {
                try!(InnerPart::addFreeBlocks(&mut space, &self.inner.path, self.inner.pgsz, self.blocks));
            }
            if !self.group_blocks.is_empty() {
                try!(InnerPart::addFreeBlocks(&mut space, &self.inner.path, self.inner.pgsz, self.group_blocks));
            }
        }
        Ok(self.list)
    }

}

// ----------------------------------------------------------------

#[cfg(remove_me)]
pub struct GenerateNumbers {
    pub cur: usize,
    pub end: usize,
    pub step: usize,
}

#[cfg(remove_me)]
impl Iterator for GenerateNumbers {
    type Item = Result<kvp>;
    // TODO allow the number of digits to be customized?
    fn next(&mut self) -> Option<Result<kvp>> {
        if self.cur > self.end {
            None
        }
        else {
            let k = format!("{:08}", self.cur).into_bytes().into_boxed_slice();
            let v = format!("{}", self.cur * 2).into_bytes().into_boxed_slice();
            let r = kvp{Key:k, Value:Blob::Array(v)};
            self.cur = self.cur + self.step;
            Some(Ok(r))
        }
    }
}

#[cfg(remove_me)]
pub struct GenerateWeirdPairs {
    pub cur: usize,
    pub end: usize,
    pub klen: usize,
    pub vlen: usize,
}

#[cfg(remove_me)]
impl Iterator for GenerateWeirdPairs {
    type Item = Result<kvp>;
    fn next(&mut self) -> Option<Result<kvp>> {
        if self.cur > self.end {
            None
        }
        else {
            fn get_weird(i: usize) -> u8 {
                let f = i as f64;
                let f = f.sin() * 1000.0;
                let f = f.abs();
                let f = f.floor() as u32;
                let f = f & 0xff;
                let f = f as u8;
                f
            }

            let mut k = Vec::new();
            for i in 0 .. self.klen {
                k.push(get_weird(i + self.cur));
            }
            let k = k.into_boxed_slice();

            let mut v = Vec::new();
            for i in 0 .. self.vlen {
                v.push(get_weird(i * 2 + self.cur));
            }
            let v = v.into_boxed_slice();

            let r = kvp{Key:k, Value:Blob::Array(v)};
            self.cur = self.cur + 1;
            Some(Ok(r))
        }
    }
}

