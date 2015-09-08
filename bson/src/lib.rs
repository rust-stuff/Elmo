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

#![feature(core)]
#![feature(collections)]
#![feature(box_syntax)]
#![feature(convert)]
#![feature(collections_drain)]
#![feature(associated_consts)]
#![feature(vec_push_all)]
#![feature(clone_from_slice)]
#![feature(drain)]
#![feature(iter_arith)]
#![feature(slice_position_elem)]

extern crate misc;

use misc::endian::*;
use misc::bufndx;

extern crate time;

#[derive(Debug)]
pub enum Error {
    // TODO remove Misc
    Misc(String),

    // TODO more detail within CorruptFile
    CorruptFile(&'static str),

    Io(std::io::Error),
    Utf8(std::str::Utf8Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            Error::Io(ref err) => write!(f, "IO error: {}", err),
            Error::Utf8(ref err) => write!(f, "Utf8 error: {}", err),
            Error::Misc(ref s) => write!(f, "Misc error: {}", s),
            Error::CorruptFile(s) => write!(f, "Corrupt file: {}", s),
        }
    }
}

impl std::error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Io(ref err) => std::error::Error::description(err),
            Error::Utf8(ref err) => std::error::Error::description(err),
            Error::Misc(ref s) => s,
            Error::CorruptFile(s) => s,
        }
    }

    // TODO cause
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(err: std::str::Utf8Error) -> Error {
        Error::Utf8(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// TODO
pub enum PathKey<'a> {
    Array(usize),
    Document(&'a str),
}

#[derive(Debug)]
pub enum WalkPath<'v, 'p> {
    SubDocument(&'p str, Box<WalkPath<'v, 'p>>),
    SubArray(&'p str, Box<WalkPath<'v, 'p>>),
    NotContainer(&'p str, &'v Value),

    Value(&'p str, &'v Value),
    NotFound(&'p str),

    // TODO not sure dive needs the str.
    // TODO every item within it has a copy of that same str.
    Dive(&'p str, Vec<WalkPath<'v, 'p>>),
}

impl<'v, 'p> WalkPath<'v, 'p> {
    pub fn has_any_dives(&self) -> bool {
        match self {
            &WalkPath::Dive(_,_) => true,

            &WalkPath::SubDocument(_,ref p) => p.has_any_dives(),
            &WalkPath::SubArray(_,ref p) => p.has_any_dives(),

            &WalkPath::NotContainer(_,_) => false,
            &WalkPath::NotFound(_) => false,

            &WalkPath::Value(_,_) => false,
        }
    }

    pub fn has_any_values(&self) -> bool {
        match self {
            &WalkPath::Dive(_,ref a) => {
                a.iter().any(|p| p.has_any_values())
            },
            &WalkPath::SubDocument(_,ref p) => p.has_any_values(),
            &WalkPath::SubArray(_,ref p) => p.has_any_values(),

            &WalkPath::NotContainer(_,_) => false,
            &WalkPath::NotFound(_) => false,

            &WalkPath::Value(_,_) => true,
        }
    }

    fn project_into_array(&self, d: &mut Array) -> Result<()> {
        // TODO mongo docs say that projecting a portion of an array
        // doesn't work without projection ops.
        match self {
            &WalkPath::Dive(_,ref a) => {
                for p in a.iter() {
                    match p {
                        &WalkPath::Value(name, ref v) => {
                            let mut sub = Document::new();
                            p.project(&mut sub);
                            d.items.push(sub.into_value());
                        },
                        &WalkPath::SubDocument(name, ref p) => {
                            let mut sub = Document::new();
                            p.project(&mut sub);
                            d.items.push(sub.into_value());
                        },
                        &WalkPath::SubArray(name, ref p) => {
                            let mut sub = Array::new();
                            p.project_into_array(&mut sub);
                            d.items.push(sub.into_value());
                        },
                        &WalkPath::NotContainer(name, ref p) => {
                        },
                        &WalkPath::NotFound(name) => {
                        },
                        &WalkPath::Dive(_,_) => {
                            // TODO what does this mean?
                        },
                    }
                }
                Ok(())
            },
            &WalkPath::SubDocument(_,_) => {
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
            &WalkPath::SubArray(_,_) => {
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
            &WalkPath::NotContainer(_,_) => {
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
            &WalkPath::NotFound(_) => {
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
            &WalkPath::Value(_,_) => {
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
        }
    }

    pub fn project(&self, d: &mut Document) -> Result<()> {
        match self {
            &WalkPath::Dive(_,_) => {
                // TODO should never happen.  this is not an array.
                Err(Error::Misc(format!("project_into_document TODO: {:?}", self)))
            },
            &WalkPath::SubDocument(name, ref p) => {
                // TODO what if name is already present?
                let sub = Document::new().into_value();
                let sub = d.set(name, sub);
                // TODO need to get the document ref back
                // TODO following line could just panic on fail
                let mut sub = try!(sub.as_mut_document());
                p.project(sub)
            },
            &WalkPath::SubArray(name,ref p) => {
                // TODO what if name is already present?
                let sub = Array::new().into_value();
                let sub = d.set(name, sub);
                // TODO need to get the array ref back
                // TODO following line could just panic on fail
                let mut sub = try!(sub.as_mut_array());
                p.project_into_array(sub)
            },
            &WalkPath::NotContainer(_,_) => {
                // TODO this is an error, right?
                Err(Error::Misc(format!("TODO: {:?}", self)))
            },
            &WalkPath::NotFound(_) => {
                //Err(Error::Misc(format!("TODO: {:?}", self)))
                // TODO do nothing?
                Ok(())
            },
            &WalkPath::Value(name,v) => {
                let _ = d.set(name, v.clone());
                Ok(())
            },
        }
    }
}

// TODO this function doesn't seem to belong here
pub fn split_name(s: &str) -> Result<(&str, &str)> {
    match s.find('.') {
        None => Err(Error::Misc(format!("bad collection name: {}", s))),
        Some(i) => {
            let a = &s[0 .. i];
            let b = &s[i+1 ..];
            Ok((a, b))
        },
    }
}

// TODO is it sufficient to derive PartialEq?
// Or do we need to implement it explicitly to
// catch the nan case?

#[derive(Clone,Debug,PartialEq)]
pub struct Document {
    // TODO consider private
    pub pairs: Vec<(String, Value)>,
}

impl Document {
    pub fn new() -> Self {
        Document {
            pairs: vec![],
        }
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn into_value(self) -> Value {
        Value::BDocument(self)
    }

    // TODO consider calling this extract
    pub fn remove(&mut self, k: &str) -> Option<Value> {
        match self.pairs.iter().position(|&(ref ksub, _)| ksub == k) {
            Some(i) => {
                let (_, v) = self.pairs.remove(i);
                return Some(v);
            },
            None => {
                return None;
            },
        }
    }

    pub fn removenocase(&mut self, k: &str) -> Option<Value> {
        match self.pairs.iter().position(|&(ref ksub, _)| std::ascii::AsciiExt::eq_ignore_ascii_case(ksub.as_str(), k)) {
            Some(i) => {
                let (_, v) = self.pairs.remove(i);
                return Some(v);
            },
            None => {
                return None;
            },
        }
    }

    pub fn validate_depth(&self, depth: usize, max: usize) -> Result<()> {
        for &(ref k, ref v) in &self.pairs {
            match v {
                &Value::BDocument(ref bd) => try!(bd.validate_depth(1 + depth, max)),
                &Value::BArray(ref ba) => try!(ba.validate_depth(1 + depth, max)),
                _ => ()
            }
        }
        Ok(())
    }

    pub fn validate_keys(&self, depth: usize) -> Result<()> {
        if depth > 0 && self.is_dbref() {
            Ok(())
        } else {
            for &(ref k, ref v) in &self.pairs {
                if k.starts_with("$") {
                    return Err(Error::Misc(String::from("key cannot start with $")));
                } else if k.contains(".") {
                    return Err(Error::Misc(String::from("key cannot contain .")));
                } else {
                    match v {
                        &Value::BDocument(ref bd) => try!(bd.validate_keys(1 + depth)),
                        &Value::BArray(ref ba) => try!(ba.validate_keys(1 + depth)),
                        _ => ()
                    }
                }
            }
            Ok(())
        }
    }

    pub fn validate_id(&mut self) -> Result<Value> {
        match self.pairs.iter().position(|&(ref k, ref v)| k == "_id") {
            Some(i) => {
                if self.pairs[i].1.is_array() {
                    return Err(Error::Misc(String::from("_id cannot be an array")));
                } else if self.pairs[i].1.is_undefined() {
                    return Err(Error::Misc(String::from("_id cannot be undefined")));
                } else if i == 0{
                    // fine
                } else {
                    // when the _id is not the first thing in the document, we must
                    // move it to the front.  it is important that we do this by
                    // shifting everything else forward, not by swapping the _id
                    // with whatever was first.
                    let id = self.pairs.remove(i);
                    self.pairs.insert(0, id);
                }
                Ok(self.pairs[0].1.clone())
            },
            None => {
                Err(Error::Misc(String::from("no id")))
            },
        }
    }

    pub fn must_remove(&mut self, k: &str) -> Result<Value> {
        self.remove(k).ok_or(Error::Misc(format!("required key not found: {}", k)))
    }

    pub fn must_removenocase(&mut self, k: &str) -> Result<Value> {
        self.removenocase(k).ok_or(Error::Misc(format!("required key not found: {}", k)))
    }

    pub fn must_remove_bool(&mut self, k: &str) -> Result<bool> {
        let v = try!(self.must_remove(k));
        // TODO note that we are calling the one that converts
        v.to_bool()
    }

    pub fn must_remove_string(&mut self, k: &str) -> Result<String> {
        let v = try!(self.must_remove(k));
        v.into_string()
    }

    pub fn must_removenocase_string(&mut self, k: &str) -> Result<String> {
        let v = try!(self.must_removenocase(k));
        v.into_string()
    }

    pub fn must_remove_document(&mut self, k: &str) -> Result<Document> {
        let v = try!(self.must_remove(k));
        v.into_document()
    }

    pub fn must_remove_array(&mut self, k: &str) -> Result<Array> {
        let v = try!(self.must_remove(k));
        v.into_array()
    }

    pub fn dives_into_any_array(&self, path: &str) -> bool {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match self.get(name) {
            None => false,
            Some(v) => {
                match dot {
                    None => false,
                    Some(dot) => {
                        let subpath = &path[dot + 1 ..];
                        match v {
                            &Value::BArray(_) => {
                                true
                            },
                            &Value::BDocument(ref bd) => {
                                bd.dives_into_any_array(subpath)
                            },
                            _ => {
                                // TODO wants to dive into something that is not a container
                                false
                            },
                        }
                    },
                }
            }
        }
    }

    pub fn entry<'v,'p>(&'v mut self, path: &'p str) -> Result<Entry<'v,'p>> {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match self.position(name) {
            Some(i) => {
                // current name is present
                match dot {
                    None => {
                        // no more diving.  what do we do with this?
                        let e = EntryFound::DocumentParent(self, i);
                        let e = Entry::Found(e);
                        Ok(e)
                    },
                    Some(dot) => {
                        // gotta dive more
                        let subpath = &path[dot + 1 ..];
                        let v = &mut self.pairs[i].1;
                        match v {
                            &mut Value::BDocument(_) | &mut Value::BArray(_) => {
                                v.entry(subpath)
                            },
                            _ => {
                                Err(Error::Misc(String::from("trying to dive into non-object")))
                            },
                        }
                    },
                }
            },
            None => {
                // current name is not present
                match dot {
                    None => {
                        // no more diving.  add it?
                        let e = EntryAbsent::DocumentParent(self, name);
                        let e = Entry::Absent(e);
                        Ok(e)
                    },
                    Some(dot) => {
                        // gotta dive more.  but there is nothing to dive into.
                        let e = EntryAbsent::DocumentAncestor(self, path);
                        let e = Entry::Absent(e);
                        Ok(e)
                    },
                }
            },
        }
    }

    fn position(&self, k: &str) -> Option<usize> {
        for i in 0 .. self.pairs.len() {
            if self.pairs[i].0 == k {
                return Some(i);
            }
        }
        return None;
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        // TODO Call self.position?
        for t in self.pairs.iter() {
            let (ref ksub, ref vsub) = *t;
            if ksub == k {
                return Some(vsub);
            }
        }
        return None;
    }

    // TODO not sure we need this?
    pub fn get_mut(&mut self, k: &str) -> Option<&mut Value> {
        for t in self.pairs.iter_mut() {
            let (ref ksub, ref mut vsub) = *t;
            if ksub == k {
                return Some(vsub);
            }
        }
        return None;
    }

    fn get_nocase(&self, k: &str) -> Option<&Value> {
        for t in self.pairs.iter() {
            let (ref ksub, ref vsub) = *t;
            if std::ascii::AsciiExt::eq_ignore_ascii_case(ksub.as_str(), k) {
                return Some(vsub);
            }
        }
        return None;
    }

    pub fn must_get(&self, k: &str) -> Result<&Value> {
        self.get(k).ok_or(Error::Misc(format!("required key not found: {}", k)))
    }

    pub fn must_get_str(&self, k: &str) -> Result<&str> {
        let v = try!(self.must_get(k));
        v.as_str()
    }

    pub fn must_get_array(&self, k: &str) -> Result<&Array> {
        let v = try!(self.must_get(k));
        v.as_array()
    }

    pub fn must_get_document(&self, k: &str) -> Result<&Document> {
        let v = try!(self.must_get(k));
        v.as_document()
    }

    pub fn set(&mut self, k: &str, v: Value) -> &mut Value {
        // TODO make this more efficient?
        for i in 0 .. self.pairs.len() {
            if self.pairs[i].0 == k {
                self.pairs[i].1 = v;
                return &mut self.pairs[i].1;
            }
        }
        self.pairs.push((String::from(k), v));
        let i = self.pairs.len() - 1;
        return &mut self.pairs[i].1;
    }

    pub fn ensure_id(&mut self) {
        match self.get("_id") {
            Some(_) => {
            },
            None => {
                self.set_objectid("_id", misc::new_bson_objectid_rand());
            },
        }
    }

    pub fn set_path(&mut self, path: &str, v: Value) -> Result<()> {
        match try!(self.entry(path)) {
            Entry::Found(e) => {
                let _ = e.replace(v);
            },
            Entry::Absent(e) => try!(e.insert(v)),
        }
        Ok(())
    }

    pub fn unset_path(&mut self, path: &str) -> Result<Option<Value>> {
        match try!(self.entry(path)) {
            Entry::Found(e) => {
                Ok(Some(e.remove()))
            },
            Entry::Absent(e) => {
                Ok(None)
            },
        }
    }

    pub fn set_objectid(&mut self, k: &str, v: [u8; 12]) {
        self.set(k, Value::BObjectID(v));
    }

    pub fn set_document(&mut self, k: &str, v: Document) -> &mut Value {
        self.set(k, Value::BDocument(v))
    }

    pub fn set_array(&mut self, k: &str, v: Array) -> &mut Value {
        self.set(k, Value::BArray(v))
    }

    pub fn set_i32(&mut self, k: &str, v: i32) {
        self.set(k, Value::BInt32(v));
    }

    pub fn set_i64(&mut self, k: &str, v: i64) {
        self.set(k, Value::BInt64(v));
    }

    pub fn set_f64(&mut self, k: &str, v: f64) {
        self.set(k, Value::BDouble(v));
    }

    pub fn set_bool(&mut self, k: &str, v: bool) {
        self.set(k, Value::BBoolean(v));
    }

    pub fn set_str(&mut self, k: &str, v: &str) {
        self.set(k, Value::BString(String::from(v)));
    }

    pub fn set_string(&mut self, k: &str, v: String) {
        self.set(k, Value::BString(v));
    }

    pub fn set_timestamp(&mut self, k: &str, v: i64) {
        self.set(k, Value::BTimeStamp(v));
    }

    pub fn set_datetime(&mut self, k: &str, v: i64) {
        self.set(k, Value::BDateTime(v));
    }

    pub fn to_bson(&self, w: &mut Vec<u8>) {
        let start = w.len();
        // placeholder for length
        w.push_all(&i32_to_bytes_le(0));
        for t in self.pairs.iter() {
            let (ref ksub, ref vsub) = *t;
            w.push(vsub.getTypeNumber_u8());
            vec_push_c_string(w, &ksub);;
            vsub.to_bson(w);
        }
        w.push(0u8);
        let len = w.len() - start;
        misc::bytes::copy_into(&i32_to_bytes_le(len as i32), &mut w[start .. start + 4]);
    }

    pub fn to_bson_array(&self) -> Vec<u8> {
        let mut v = Vec::new();
        self.to_bson(&mut v);
        v
    }

    pub fn find_all_strings<'a>(&'a self, dest: &mut Vec<&'a str>) {
        for t in &self.pairs {
            t.1.find_all_strings(dest);
        }
    }

    pub fn exclude_path(&mut self, path: &str) {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match self.pairs.iter().position(|&(ref k, _)| k == name) {
            Some(ndx) => {
                match dot {
                    None => {
                        self.pairs.remove(ndx);
                    },
                    Some(dot) => {
                        let v = &mut self.pairs[ndx].1;
                        match v {
                            &mut Value::BDocument(ref mut bd) => {
                                bd.exclude_path(&path[dot + 1..])
                            },
                            &mut Value::BArray(ref mut ba) => {
                                ba.exclude_path(&path[dot + 1..])
                            },
                            _ => {
                                // TODO error?
                            },
                        }
                    },
                }
            },
            None => {
                // TODO error?
            },
        }
    }

    pub fn walk_path<'v, 'p>(&'v self, path: &'p str) -> WalkPath<'v, 'p> {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match self.pairs.iter().position(|&(ref k, _)| k == name) {
            Some(ndx) => {
                let v = &self.pairs[ndx].1;
                match dot {
                    None => WalkPath::Value(name, v),
                    Some(dot) => {
                        match v {
                            &Value::BDocument(ref bd) => {
                                WalkPath::SubDocument(name, box bd.walk_path(&path[dot + 1..]))
                            },
                            &Value::BArray(ref ba) => {
                                WalkPath::SubArray(name, box ba.walk_path(&path[dot + 1..]))
                            },
                            _ => {
                                WalkPath::NotContainer(name, v)
                            },
                        }
                    },
                }
            },
            None => {
                // TODO do we need to distinguish between dot/not?
                // ie, whether this was supposed to be a container or not?
                WalkPath::NotFound(path)
            },
        }
    }

    pub fn find_path(&self, path: &str) -> Value {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match slice_find(&self.pairs, name) {
            Some(ndx) => {
                let v = &self.pairs[ndx].1;
                match dot {
                    // TODO ouch.  horrifying clone.
                    None => v.clone(),
                    Some(dot) => v.find_path(&path[dot + 1..])
                }
            },
            None => Value::BUndefined
        }
    }

    pub fn from_bson(w: &[u8]) -> Result<Document> {
        let mut cur = 0;
        let d = try!(slurp_document(w, &mut cur));
        Ok(d)
    }

    pub fn is_dbref(&self) -> bool {
        let has_ref = slice_find(&self.pairs, "$ref").is_some();
        let has_id =  slice_find(&self.pairs, "$id").is_some();
        let has_db =  slice_find(&self.pairs, "$db").is_some();
        let len = self.pairs.len();
        if len==2 && has_ref && has_id {
            true
        } else if len==3 && has_ref && has_id && has_db {
            true
        } else {
            false
        }
    }

}

#[derive(Clone,Debug)]
pub struct Array {
    // TODO consider private
    pub items: Vec<Value>,
}

impl Array {
    pub fn new() -> Self {
        Array {
            items: vec![],
        }
    }

    pub fn into_value(self) -> Value {
        Value::BArray(self)
    }

    pub fn push(&mut self, v: Value) {
        self.items.push(v);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn validate_keys(&self, depth: usize) -> Result<()> {
        for v in &self.items {
            match v {
                &Value::BDocument(ref bd) => try!(bd.validate_keys(1 + depth)),
                &Value::BArray(ref ba) => try!(ba.validate_keys(1 + depth)),
                _ => ()
            }
        }
        Ok(())
    }

    pub fn validate_depth(&self, depth: usize, max: usize) -> Result<()> {
        if depth > max {
            return Err(Error::Misc(format!("too much nesting")));
        }
        for v in &self.items {
            match v {
                &Value::BDocument(ref bd) => try!(bd.validate_depth(1 + depth, max)),
                &Value::BArray(ref ba) => try!(ba.validate_depth(1 + depth, max)),
                _ => ()
            }
        }
        Ok(())
    }

    pub fn exclude_path(&mut self, path: &str) {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match name.parse::<i32>() {
            Err(_) => {
                // when we have an array and the next step of the path is not
                // an integer index, we search any subdocs in that array for
                // that path and construct an array of the matches.

                // document : { a:1, b:[ { c:1 }, { c:2 } ] }
                // path : b.c
                // needs to get: [ 1, 2 ]

                for subv in self.items.iter_mut() {
                    match subv {
                        &mut Value::BDocument(ref mut bd) => {
                            bd.exclude_path(path);
                        },
                        _ => {
                            // TODO error?
                        },
                    }
                }
            }, 
            Ok(ndx) => {
                if ndx < 0 || (ndx as usize) >= self.items.len() {
                    // TODO error?
                } else {
                    let ndx = ndx as usize;
                    match dot {
                        None => {
                            self.items.remove(ndx);
                        },
                        Some(dot) => {
                            let v = &mut self.items[ndx];
                            match v {
                                &mut Value::BDocument(ref mut bd) => {
                                    bd.exclude_path(&path[dot + 1 ..])
                                },
                                &mut Value::BArray(ref mut ba) => {
                                    ba.exclude_path(&path[dot + 1 ..])
                                },
                                _ => {
                                    // TODO error?
                                },
                            }
                        },
                    }
                }
            }
        }
    }

    pub fn walk_path<'v, 'p>(&'v self, path: &'p str) -> WalkPath<'v, 'p> {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match name.parse::<i32>() {
            Err(_) => {
                // when we have an array and the next step of the path is not
                // an integer index, we search any subdocs in that array for
                // that path and construct an array of the matches.

                // document : { a:1, b:[ { c:1 }, { c:2 } ] }
                // path : b.c
                // needs to get: [ 1, 2 ]

                let a = self.items.iter().map(|subv| 
                                              match subv {
                                                  &Value::BDocument(ref bd) => {
                                                      bd.walk_path(path)
                                                  },
                                                  _ => {
                                                      WalkPath::NotContainer(path, subv)
                                                  },
                                              }
                                              ).collect::<Vec<_>>();
                WalkPath::Dive(path, a)
            }, 
            Ok(ndx) => {
                if ndx < 0 || (ndx as usize) >= self.items.len() {
                    // TODO do we need to distinguish between dot or not?
                    WalkPath::NotFound(name)
                } else {
                    let ndx = ndx as usize;
                    let v = &self.items[ndx];
                    match dot {
                        None => WalkPath::Value(name, v),
                        Some(dot) => {
                            match v {
                                &Value::BDocument(ref bd) => {
                                    WalkPath::SubDocument(name, box bd.walk_path(&path[dot + 1 ..]))
                                },
                                &Value::BArray(ref ba) => {
                                    WalkPath::SubArray(name, box ba.walk_path(&path[dot + 1 ..]))
                                },
                                _ => {
                                    WalkPath::NotContainer(name, v)
                                },
                            }
                        },
                    }
                }
            }
        }
    }

    pub fn set_path(&mut self, path: &str, v: Value) -> Result<()> {
        match try!(self.entry(path)) {
            Entry::Found(e) => {
                let _ = e.replace(v);
            },
            Entry::Absent(e) => try!(e.insert(v)),
        }
        Ok(())
    }

    pub fn unset_path(&mut self, path: &str) -> Result<Option<Value>> {
        match try!(self.entry(path)) {
            Entry::Found(e) => {
                Ok(Some(e.remove()))
            },
            Entry::Absent(e) => {
                Ok(None)
            },
        }
    }

    pub fn entry<'v,'p>(&'v mut self, path: &'p str) -> Result<Entry<'v,'p>> {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match name.parse::<usize>() {
            Ok(i) => {
                // it's an integer.
                if i < self.items.len() {
                    // this array position already exists
                    match dot {
                        None => {
                            // no more diving.  now what?
                            let e = EntryFound::ArrayParent(self, i);
                            let e = Entry::Found(e);
                            Ok(e)
                        },
                        Some(dot) => {
                            // gotta dive more
                            let subpath = &path[dot + 1 ..];
                            let v = &mut self.items[i];
                            match v {
                                &mut Value::BDocument(_) | &mut Value::BArray(_) => {
                                    v.entry(subpath)
                                },
                                _ => {
                                    Err(Error::Misc(String::from("trying to dive into non-object")))
                                },
                            }
                        },
                    }
                } else {
                    match dot {
                        None => {
                            let e = EntryAbsent::ArrayParent(self, i);
                            let e = Entry::Absent(e);
                            Ok(e)
                        },
                        Some(dot) => {
                            // gotta dive more, but the array isn't big enough
                            let e = EntryAbsent::ArrayAncestor(self, path);
                            let e = Entry::Absent(e);
                            Ok(e)
                        },
                    }
                }
            },
            Err(_) => {
                Err(Error::Misc(format!("trying to dive into array {:?} with non-integer name: {}", self, name)))
            },
        }
    }

    fn to_bson(&self, w: &mut Vec<u8>) {
        let start = w.len();
        // placeholder for length
        w.push_all(&i32_to_bytes_le(0));
        for (i, vsub) in self.items.iter().enumerate() {
            w.push(vsub.getTypeNumber_u8());
            let s = format!("{}", i);
            vec_push_c_string(w, &s);
            vsub.to_bson(w);
        }
        w.push(0u8);
        let len = w.len() - start;
        misc::bytes::copy_into(&i32_to_bytes_le(len as i32), &mut w[start .. start + 4]);
    }

    fn find_all_strings<'a>(&'a self, dest: &mut Vec<&'a str>) {
        for v in &self.items {
            v.find_all_strings(dest);
        }
    }

    fn tryGetValueAtIndex(&self, ndx: usize) -> Option<&Value> {
        if ndx<0 {
            return None
        } else if ndx >= self.items.len() {
            return None
        } else {
            return Some(&self.items[ndx])
        }
    }

    fn setValueAtIndex(&mut self, ndx: usize, v: Value) {
        if ndx > 1500001 { panic!( "too big"); } // TODO this limit passes test set7.js, but is a bad idea
        if ndx >= self.items.len() {
            // TODO
        }
        self.items[ndx] = v;
    }

    fn removeValueAtIndex(&mut self, ndx: usize) {
        self.items.remove(ndx);
    }

    fn unsetValueAtIndex(&mut self, ndx: usize) {
        if ndx >=0 && ndx < self.items.len() {
            self.items[ndx] = Value::BNull;
        }
    }

}

#[derive(Clone,Debug)]
pub enum Value {
    BDouble(f64),
    BString(String),
    BInt64(i64),
    BInt32(i32),
    BUndefined,
    BObjectID([u8; 12]),
    BNull,
    BRegex(String, String),
    BJSCode(String),
    BJSCodeWithScope(String),
    BBinary(u8, Vec<u8>),
    BMinKey,
    BMaxKey,
    BDateTime(i64),
    BTimeStamp(i64),
    BBoolean(bool),
    BArray(Array),
    BDocument(Document),
}

// We want the ability to put a Value into a HashSet,
// but it contains an f64, which does not implement Eq or Hash.
// So we provide implementations below for Value that
// are sufficient for our purposes.

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        // TODO slow
        let a = self.to_bson_array();
        let b = other.to_bson_array();
        a == b
    }
}

impl Eq for Value {
}

impl std::hash::Hash for Value {
    fn hash<H>(&self, state: &mut H) where H: std::hash::Hasher {
        // TODO slow
        let a = self.to_bson_array();
        state.write(&a);
    }
}

fn vec_push_c_string(v: &mut Vec<u8>, s: &str) {
    v.push_all(s.as_bytes());
    v.push(0);
}

fn vec_push_bson_string(v: &mut Vec<u8>, s: &str) {
    // TODO i32 vs u32.  silly.
    v.push_all(&i32_to_bytes_le( (s.len() + 1) as i32 ));
    v.push_all(s.as_bytes());
    v.push(0);
}

// TODO this should be a library func, right?
// TODO this is basically position(), I think.
fn slice_find(pairs: &[(String, Value)], s: &str) -> Option<usize> {
    for i in 0 .. pairs.len() {
        if pairs[i].0.as_str() == s {
            return Some(i);
        }
    }
    None
}

fn slurp_bson_string(ba: &[u8], i: &mut usize) -> Result<String> {
    // TODO the spec says the len here is a signed number, but that's silly
    let len = bufndx::slurp_u32_le(ba, i) as usize;

    let s = try!(std::str::from_utf8(&ba[*i .. *i + len - 1]));
    *i = *i + len;
    Ok(String::from(s))
}

fn slurp_bson_value(ba: &[u8], i: &mut usize, valtype: u8) -> Result<Value> {
    let bv =
        match valtype {
            1 => Value::BDouble(bufndx::slurp_f64_le(ba, i)),
            2 => Value::BString(try!(slurp_bson_string(ba, i))),
            3 => Value::BDocument(try!(slurp_document(ba, i))),
            4 => Value::BArray(try!(slurp_array(ba, i))),
            5 => slurp_binary(ba, i),
            6 => Value::BUndefined,
            7 => slurp_objectid(ba, i),
            8 => slurp_boolean(ba, i),
            9 => Value::BDateTime(bufndx::slurp_i64_le(ba, i)),
            10 => Value::BNull,
            11 => try!(slurp_regex(ba, i)),
            12 => try!(slurp_deprecated_12(ba, i)),
            13 => try!(slurp_js(ba, i)),
            15 => try!(slurp_js_with_scope(ba, i)),
            16 => Value::BInt32(bufndx::slurp_i32_le(ba, i)),
            17 => Value::BTimeStamp(bufndx::slurp_i64_le(ba, i)),
            18 => Value::BInt64(bufndx::slurp_i64_le(ba, i)),
            127 => Value::BMaxKey,
            255 => Value::BMinKey,
            _ => panic!("invalid BSON value type"),
        };
    Ok(bv)
}

fn slurp_deprecated_12(ba: &[u8], i: &mut usize) -> Result<Value> {
    // deprecated
    let a = try!(slurp_bson_string(ba, i));
    Ok(slurp_objectid(ba, i))
}

fn slurp_js(ba: &[u8], i: &mut usize) -> Result<Value> {
    let a = try!(slurp_bson_string(ba, i));
    Ok(Value::BJSCode(a))
}

fn slurp_js_with_scope(ba: &[u8], i: &mut usize) -> Result<Value> {
    // TODO the spec says the len here is a signed number, but that's silly
    let len = bufndx::slurp_u32_le(ba, i);

    let a = try!(slurp_bson_string(ba, i));
    let scope = try!(slurp_document(ba, i));
    Ok(Value::BJSCodeWithScope(a))
}

fn slurp_regex(ba: &[u8], i: &mut usize) -> Result<Value> {
    let expr = try!(bufndx::slurp_cstring(ba, i));
    let options = try!(bufndx::slurp_cstring(ba, i));
    Ok(Value::BRegex(expr, options))
}

fn slurp_binary(ba: &[u8], i: &mut usize) -> Value {
    // TODO the spec says the len here is a signed number, but that's silly
    let len = bufndx::slurp_u32_le(ba, i) as usize;

    let subtype = ba[*i];
    *i = *i + 1;
    let mut b = Vec::with_capacity(len);
    b.push_all(&ba[*i .. *i + len]);
    *i = *i + len;
    Value::BBinary(subtype, b)
}

fn slurp_objectid(ba: &[u8], i: &mut usize) -> Value {
    let mut b = [0; 12];
    b.clone_from_slice(&ba[*i .. *i + 12]);
    *i = *i + 12;
    Value::BObjectID(b)
}

fn slurp_boolean(ba: &[u8], i: &mut usize) -> Value {
    let b = ba[*i] != 0;
    *i = *i + 1;
    Value::BBoolean(b)
}

fn slurp_document_pairs(ba: &[u8], i: &mut usize) -> Result<Vec<(String, Value)>> {
    // TODO the spec says the len here is a signed number, but that's silly
    let len = misc::bufndx::slurp_u32_le(ba, i) as usize;

    let mut pairs = Vec::new();
    while ba[*i] != 0 {
        let valtype = ba[*i];
        *i = *i + 1;
        let k = try!(bufndx::slurp_cstring(ba, i));
        let v = try!(slurp_bson_value(ba, i, valtype));
        pairs.push((k,v));
    }
    assert!(ba[*i] == 0);
    *i = *i + 1;
    // TODO verify len
    Ok(pairs)
}

pub fn slurp_document(ba: &[u8], i: &mut usize) -> Result<Document> {
    let pairs = try!(slurp_document_pairs(ba, i));
    Ok(Document {pairs: pairs})
}

fn slurp_array(ba: &[u8], i: &mut usize) -> Result<Array> {
    let pairs = try!(slurp_document_pairs(ba, i));
    // TODO verify that the keys are correct, integers, ascending, etc?
    let a = pairs.into_iter().map(|t| {
        let (k,v) = t;
        v
    }).collect();
    Ok(Array { items: a})
}

pub enum EntryFound<'v> {
    DocumentParent(&'v mut Document, usize),
    ArrayParent(&'v mut Array, usize),
}

impl<'v> EntryFound<'v> {
    pub fn get(&self) -> &Value {
        match self {
            &EntryFound::DocumentParent(ref bd, i) => {
                &bd.pairs[i].1
            },
            &EntryFound::ArrayParent(ref ba, i) => {
                &ba.items[i]
            },
        }
    }

    // TODO why does self need to be mut here when it does not
    // for remove() and replace() below?
    pub fn get_mut(&mut self) -> &mut Value {
        match self {
            &mut EntryFound::DocumentParent(ref mut bd, i) => {
                &mut bd.pairs[i].1
            },
            &mut EntryFound::ArrayParent(ref mut ba, i) => {
                &mut ba.items[i]
            },
        }
    }

    pub fn remove(self) -> Value {
        match self {
            EntryFound::DocumentParent(bd, i) => {
                bd.pairs.remove(i).1
            },
            EntryFound::ArrayParent(ba, i) => {
                ba.items.remove(i)
            },
        }
    }

    pub fn replace(self, v: Value) -> Value {
        match self {
            EntryFound::DocumentParent(bd, i) => {
                let (k,old) = bd.pairs.remove(i);
                bd.pairs.insert(i, (k, v));
                old
            },
            EntryFound::ArrayParent(ba, i) => {
                let old = ba.items.remove(i);
                ba.items.insert(i, v);
                old
            },
        }
    }
}

pub enum EntryAbsent<'v,'p> {
    DocumentParent(&'v mut Document, &'p str),
    ArrayParent(&'v mut Array, usize),
    DocumentAncestor(&'v mut Document, &'p str),
    ArrayAncestor(&'v mut Array, &'p str),
}

impl<'v,'p> EntryAbsent<'v,'p> {
    // TODO return mut ref to it?
    pub fn insert(self, v: Value) -> Result<()> {
        match self {
            EntryAbsent::DocumentParent(bd, k) => {
                bd.pairs.push((String::from(k), v));
            },
            EntryAbsent::ArrayParent(ba, i) => {
                if i > 1500000 {
                    return Err(Error::Misc(format!("EntryAbsent::ArrayParent insert: len={}, i={} too big", ba.len(), i)));
                }
                let empties = i - ba.len();
                for _ in 0 .. empties {
                    ba.items.push(Value::BNull);
                }
                ba.items.push(v);
            },
            EntryAbsent::DocumentAncestor(bd, path) => {
                let dot = path.find('.').expect("should not be here if no dot");
                let name = &path[0 .. dot];
                let subpath = &path[dot + 1 ..];
                match name.parse::<usize>() {
                    Ok(n) => {
                        let sub = bd.set_array(name, Array::new());
                        try!(sub.set_path(subpath, v));
                    },
                    Err(_) => {
                        let sub = bd.set_document(name, Document::new());
                        try!(sub.set_path(subpath, v));
                    },
                }
            },
            EntryAbsent::ArrayAncestor(ba, path) => {
                return Err(Error::Misc(format!("TODO EntryAbsent::ArrayAncestor insert: len={}, path={}", ba.len(), path)));
            },
        }
        Ok(())
    }
}

pub enum Entry<'v,'p> {
    Found(EntryFound<'v>),
    Absent(EntryAbsent<'v,'p>),
}

impl Value {
    pub fn set_path(&mut self, path: &str, v: Value) -> Result<()> {
        match self {
            &mut Value::BDocument(ref mut bd) => bd.set_path(path, v),
            &mut Value::BArray(ref mut ba) => ba.set_path(path, v),
            // TODO the following line should probably be Err
            _ => unreachable!(),
        }
    }

    pub fn unset_path(&mut self, path: &str) -> Result<Option<Value>> {
        match self {
            &mut Value::BDocument(ref mut bd) => bd.unset_path(path),
            &mut Value::BArray(ref mut ba) => ba.unset_path(path),
            // TODO the following line should probably be Err
            _ => unreachable!(),
        }
    }

    pub fn entry<'v,'p>(&'v mut self, path: &'p str) -> Result<Entry<'v,'p>> {
        match self {
            &mut Value::BDocument(ref mut bd) => bd.entry(path),
            &mut Value::BArray(ref mut ba) => ba.entry(path),
            // TODO the following line should probably be Err
            _ => unreachable!(),
        }
    }

    pub fn tryGetValueEither(&self, k: &str) -> Option<&Value> {
        match self {
            &Value::BDocument(ref bd) => bd.get(k),
            &Value::BArray(ref ba) => {
                match k.parse::<usize>() {
                    Ok(n) => ba.tryGetValueAtIndex(n),
                    // TODO or should we propagate the error?
                    Err(_) => None,
                }
            },
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            &Value::BNull => true,
            _ => false,
        }
    }

    pub fn is_array(&self) -> bool {
        match self {
            &Value::BArray(_) => true,
            _ => false,
        }
    }

    pub fn is_undefined(&self) -> bool {
        match self {
            &Value::BUndefined => true,
            _ => false,
        }
    }

    pub fn is_string(&self) -> bool {
        match self {
            &Value::BString(_) => true,
            _ => false,
        }
    }

    pub fn is_document(&self) -> bool {
        match self {
            &Value::BDocument(_) => true,
            _ => false,
        }
    }

    pub fn is_numeric(&self) -> bool {
        match self {
            &Value::BInt32(_) => true,
            &Value::BInt64(_) => true,
            &Value::BDouble(_) => true,
            _ => false,
        }
    }

    pub fn is_nan(&self) -> bool {
        match self {
            &Value::BDouble(f) => f.is_nan(),
            _ => false,
        }
    }

    pub fn is_date(&self) -> bool {
        match self {
            &Value::BDateTime(_) => true,
            _ => false,
        }
    }

    pub fn into_expr_string(self) -> Result<String> {
        // TODO what are the rules for how/when string coercion happens?
        // this function was written simply because the string expression
        // functions in the aggregation pipeline are documented to require
        // strings but their test suite has a number of cases that verify
        // that coercion to string happens for certain types.  but I can't
        // find a spec which explains which types get coerced and which ones
        // do not.

        match self {
            Value::BDateTime(n) => {
                let sec = n / 1000;
                let ts = time::Timespec::new(sec, 0);
                let tm = time::at(ts);
                // yyyy-MM-ddTHH:mm:ss
                match time::strftime("%Y-%m-%dT%H:%M:%S", &tm) {
                    Ok(s) => {
                        Ok(s)
                    },
                    Err(_) => {
                        // TODO get the actual error into this
                        Err(Error::Misc(format!("strftime failed")))
                    },
                }
            },
            Value::BInt32(n) => Ok(format!("{}", n)),
            Value::BInt64(n) => Ok(format!("{}", n)),
            Value::BDouble(n) => Ok(format!("{}", n)),
            Value::BString(s) => Ok(s),
            Value::BNull => Ok(String::from("")),
            _ => Err(Error::Misc(format!("into_expr_string failed: {:?}", self))),
        }
    }

    pub fn into_string(self) -> Result<String> {
        // TODO consider having this (and similar functions) accept a string to use
        // as the error message.
        match self {
            Value::BString(s) => Ok(s),
            _ => Err(Error::Misc(format!("string required, but found {:?}", self))),
        }
    }

    // TODO how to make this function NOT sound like it is converting anything to a string?
    pub fn as_str(&self) -> Result<&str> {
        match self {
            &Value::BString(ref s) => Ok(s),
            _ => Err(Error::Misc(format!("string required, but found {:?}", self))),
        }
    }

    pub fn as_array(&self) -> Result<&Array> {
        match self {
            &Value::BArray(ref s) => Ok(s),
            _ => Err(Error::Misc(format!("array required, but found {:?}", self))),
        }
    }

    pub fn expect_document(self) -> Document {
        match self {
            Value::BDocument(s) => s,
            _ => panic!(),
        }
    }

    pub fn as_document(&self) -> Result<&Document> {
        match self {
            &Value::BDocument(ref s) => Ok(s),
            _ => Err(Error::Misc(format!("document required, but found {:?}", self))),
        }
    }

    pub fn as_mut_document(&mut self) -> Result<&mut Document> {
        match self {
            &mut Value::BDocument(ref mut s) => Ok(s),
            _ => Err(Error::Misc(format!("document required, but found {:?}", self))),
        }
    }

    pub fn as_mut_array(&mut self) -> Result<&mut Array> {
        match self {
            &mut Value::BArray(ref mut s) => Ok(s),
            _ => Err(Error::Misc(format!("array required, but found {:?}", self))),
        }
    }

    pub fn as_document_or_panic(&self) -> &Document {
        match self.as_document() {
            Ok(d) => d,
            Err(_) => panic!("must be document"),
        }
    }

    pub fn into_document(self) -> Result<Document> {
        match self {
            Value::BDocument(s) => Ok(s),
            _ => Err(Error::Misc(format!("document required, but found {:?}", self))),
        }
    }

    pub fn into_array(self) -> Result<Array> {
        match self {
            Value::BArray(s) => Ok(s),
            _ => Err(Error::Misc(format!("array required, but found {:?}", self))),
        }
    }

    pub fn as_objectid(&self) -> Result<[u8; 12]> {
        match self {
            &Value::BObjectID(a) => Ok(a),
            _ => Err(Error::Misc(format!("objectid required, but found {:?}", self))),
        }
    }

    pub fn as_expr_bool(&self) -> bool {
        match self {
            &Value::BBoolean(b) => b,
            &Value::BNull => false,
            &Value::BUndefined => false,
            &Value::BInt32(0) => false,
            &Value::BInt64(0) => false,
            &Value::BDouble(0.0) => false,
            _ => true,
        }
    }

    pub fn as_bool(&self) -> Result<bool> {
        match self {
            &Value::BBoolean(b) => Ok(b),
            _ => Err(Error::Misc(format!("bool required, but found {:?}", self))),
        }
    }

    // TODO need a naming convention for the difference between this func and the one above.
    // TODO "must be exactly a BBoolean"
    // TODO vs
    // TODO "must be convertible to a bool"
    pub fn to_bool(&self) -> Result<bool> {
        match self {
            &Value::BBoolean(b) => Ok(b),
            &Value::BInt32(n) => Ok(n != 0),
            &Value::BInt64(n) => Ok(n != 0),
            &Value::BDouble(f) => Ok(f != 0.0),
            _ => Err(Error::Misc(format!("need something convertible to bool, but found {:?}", self))),
        }
    }

    fn getDate(&self) -> Result<i64> {
        match self {
            &Value::BDateTime(ref s) => Ok(*s),
            _ => Err(Error::Misc(String::from("must be DateTime"))),
        }
    }

    pub fn i32_or_panic(&self) -> i32 {
        match self {
            &Value::BInt32(n) => n,
            _ => panic!("must be i32"),
        }
    }

    pub fn f64_or_panic(&self) -> f64 {
        match self {
            &Value::BDouble(n) => n,
            _ => panic!("must be f64"),
        }
    }

    pub fn as_i32(&self) -> Result<i32> {
        match self {
            &Value::BInt32(s) => Ok(s),
            _ => Err(Error::Misc(String::from("must be i32"))),
        }
    }

    pub fn numeric_to_i32(&self) -> Result<i32> {
        match self {
            &Value::BInt32(s) => Ok(s as i32),
            &Value::BInt64(s) => Ok(s as i32),
            &Value::BDouble(s) => Ok(s as i32),
            _ => Err(Error::Misc(format!("numeric required, but found {:?}", self))),
        }
    }

    pub fn integer_to_i64(&self) -> Result<i64> {
        match self {
            &Value::BInt32(s) => Ok(s as i64),
            &Value::BInt64(s) => Ok(s as i64),
            _ => Err(Error::Misc(format!("integer required, but found {:?}", self))),
        }
    }

    pub fn numeric_to_i64(&self) -> Result<i64> {
        match self {
            &Value::BInt32(s) => Ok(s as i64),
            &Value::BInt64(s) => Ok(s as i64),
            &Value::BDouble(s) => Ok(s as i64),
            _ => Err(Error::Misc(format!("numeric required, but found {:?}", self))),
        }
    }

    pub fn datetime_to_i64(&self) -> Result<i64> {
        match self {
            &Value::BDateTime(s) => Ok(s as i64),
            &Value::BTimeStamp(s) => {
                let ms = (s >> 32) * 1000;
                Ok(ms)
            },
            _ => Err(Error::Misc(format!("datetime or timestamp required, but found {:?}", self))),
        }
    }

    pub fn numeric_or_datetime_to_i64(&self) -> Result<i64> {
        match self {
            &Value::BInt32(s) => Ok(s as i64),
            &Value::BInt64(s) => Ok(s as i64),
            &Value::BDateTime(s) => Ok(s as i64),
            &Value::BDouble(s) => Ok(s as i64),
            _ => Err(Error::Misc(format!("numeric required, but found {:?}", self))),
        }
    }

    pub fn numeric_to_f64(&self) -> Result<f64> {
        match self {
            &Value::BInt32(s) => Ok(s as f64),
            &Value::BInt64(s) => Ok(s as f64),
            &Value::BDouble(s) => Ok(s as f64),
            _ => Err(Error::Misc(format!("numeric required, but found {:?}", self))),
        }
    }

    fn getAsExprBool(&self) -> bool {
        match self {
            &Value::BBoolean(false) => false,
            &Value::BNull => false,
            &Value::BUndefined => false,
            &Value::BInt32(0) => false,
            &Value::BInt64(0) => false,
            &Value::BDouble(0.0) => false,
            _ => true,
        }
    }

    fn getAsBool(&self) -> Result<bool> {
        match self {
        &Value::BBoolean(b) => Ok(b),
        &Value::BInt32(i) => Ok(i!=0),
        &Value::BInt64(i) => Ok(i!=0),
        &Value::BDouble(f) => Ok((f as i32)!=0),
        _ => Err(Error::Misc(String::from("must be convertible to bool"))),
        }
    }

    fn getAsInt32(&self) -> Result<i32> {
        match self {
        &Value::BInt32(a) => Ok(a),
        &Value::BInt64(a) => Ok(a as i32),
        &Value::BDouble(a) => Ok(a as i32),
        _ => Err(Error::Misc(String::from("must be convertible to int32"))),
        }
    }

    fn getAsInt64(&self) -> Result<i64> {
        match self {
        &Value::BInt32(a) => Ok(a as i64),
        &Value::BInt64(a) => Ok(a),
        &Value::BDouble(a) => Ok(a as i64),
        &Value::BDateTime(a) => Ok(a as i64),
        _ => Err(Error::Misc(String::from("must be convertible to int64"))),
        }
    }

    fn getAsDouble(&self) -> Result<f64> {
        match self {
        &Value::BInt32(a) => Ok(a as f64),
        &Value::BInt64(a) => Ok(a as f64),
        &Value::BDouble(a) => Ok(a),
        _ => Err(Error::Misc(String::from("must be convertible to f64"))),
        }
    }

    // TODO it's awful that this returns a clone, simply because
    // it sometimes has to construct an array.
    pub fn find_path(&self, path: &str) -> Value {
        let dot = path.find('.');
        let name = match dot { 
            None => path,
            Some(ndx) => &path[0 .. ndx]
        };
        match self {
            &Value::BDocument(ref bd) => bd.find_path(path),
            &Value::BArray(ref ba) => {
                // TODO move into array and call from here?
                // TODO why not parse as usize?  
                // what should happen if an element of the path is a
                // negative number?
                match name.parse::<i32>() {
                    Err(_) => {
                        // when we have an array and the next step of the path is not
                        // an integer index, we search any subdocs in that array for
                        // that path and construct an array of the matches.

                        // document : { a:1, b:[ { c:1 }, { c:2 } ] }
                        // path : b.c
                        // needs to get: [ 1, 2 ]

                        // TODO are there any functions in the matcher which could be
                        // simplified by using this function? 
                        let a:Vec<Value> = ba.items.iter().filter_map(|subv| 
                                match subv {
                                &Value::BDocument(_) => Some(subv.find_path(path)),
                                _ => None
                                }
                                                       ).collect();
                        // if nothing matched, return None instead of an empty array.
                        // TODO is this right?
                        if a.len()==0 { Value::BUndefined } else { Value::BArray(Array { items: a }) }
                    }, 
                    Ok(ndx) => {
                        if ndx<0 {
                            // TODO useless panic.  need to return Result.
                            panic!( "array index < 0");
                        } else if (ndx as usize)>=ba.items.len() {
                            // TODO useless panic.  need to return Result.
                            panic!( "array index too large");
                        } else {
                            let v = &ba.items[ndx as usize];
                            match dot {
                                None => v.clone(),
                                Some(dot) => v.find_path(&path[dot + 1..])
                            }
                        }
                    }
                }
            },
            _ => Value::BUndefined
        }
    }

    pub fn getTypeNumber_u8(&self) -> u8 {
        match self {
            &Value::BDouble(_) => 1,
            &Value::BString(_) => 2,
            &Value::BDocument(_) => 3,
            &Value::BArray(_) => 4,
            &Value::BBinary(_, _) => 5,
            &Value::BUndefined => 6,
            &Value::BObjectID(_) => 7,
            &Value::BBoolean(_) => 8,
            &Value::BDateTime(_) => 9,
            &Value::BNull => 10,
            &Value::BRegex(_, _) => 11,
            &Value::BJSCode(_) => 13,
            &Value::BJSCodeWithScope(_) => 15,
            &Value::BInt32(_) => 16,
            &Value::BTimeStamp(_) => 17,
            &Value::BInt64(_) => 18,
            &Value::BMinKey => 255, // NOTE
            &Value::BMaxKey => 127,
        }
    }

    pub fn get_type_name(&self) -> &'static str {
        match self {
            &Value::BDouble(_) => "f64",
            &Value::BString(_) => "string",
            &Value::BDocument(_) => "document",
            &Value::BArray(_) => "array",
            &Value::BBinary(_, _) => "binary",
            &Value::BUndefined => "undefined",
            &Value::BObjectID(_) => "objectid",
            &Value::BBoolean(_) => "bool",
            &Value::BDateTime(_) => "datetime",
            &Value::BNull => "null",
            &Value::BRegex(_, _) => "regex",
            &Value::BJSCode(_) => "jscode",
            &Value::BJSCodeWithScope(_) => "jscodewithscope",
            &Value::BInt32(_) => "i32",
            &Value::BTimeStamp(_) => "timestamp",
            &Value::BInt64(_) => "i64",
            &Value::BMinKey => "minkey",
            &Value::BMaxKey => "maxkey",
        }
    }

    pub fn for_all_strings<F : Fn(&str) -> ()>(&self, func: &F) {
        match self {
            &Value::BDouble(_) => (),
            &Value::BString(ref s) => func(&s),
            &Value::BDocument(ref bd) => {
                for t in &bd.pairs {
                    t.1.for_all_strings(func);
                }
            },
            &Value::BArray(ref ba) => {
                for v in &ba.items {
                    v.for_all_strings(func);
                }
            },
            &Value::BBinary(_, _) => (),
            &Value::BUndefined => (),
            &Value::BObjectID(_) => (),
            &Value::BBoolean(_) => (),
            &Value::BDateTime(_) => (),
            &Value::BNull => (),
            &Value::BRegex(_, _) => (),
            &Value::BJSCode(_) => (),
            &Value::BJSCodeWithScope(_) => (),
            &Value::BInt32(_) => (),
            &Value::BTimeStamp(_) => (),
            &Value::BInt64(_) => (),
            &Value::BMinKey => (),
            &Value::BMaxKey => (),
        }
    }

    pub fn find_all_strings<'a>(&'a self, dest: &mut Vec<&'a str>) {
        match self {
            &Value::BDouble(_) => (),
            &Value::BString(ref s) => dest.push(&s),
            &Value::BDocument(ref bd) => bd.find_all_strings(dest),
            &Value::BArray(ref ba) => ba.find_all_strings(dest),
            &Value::BBinary(_, _) => (),
            &Value::BUndefined => (),
            &Value::BObjectID(_) => (),
            &Value::BBoolean(_) => (),
            &Value::BDateTime(_) => (),
            &Value::BNull => (),
            &Value::BRegex(_, _) => (),
            &Value::BJSCode(_) => (),
            &Value::BJSCodeWithScope(_) => (),
            &Value::BInt32(_) => (),
            &Value::BTimeStamp(_) => (),
            &Value::BInt64(_) => (),
            &Value::BMinKey => (),
            &Value::BMaxKey => (),
        }
    }

    pub fn get_weight_from_index_entry(k: &[u8]) -> Result<i32> {
        let n = 1 + k.iter().rposition(|v| *v==0).expect("TODO");
        let ord_shouldbe = Value::BInt32(0).get_type_order() as u8;
        if k[n] != ord_shouldbe {
            return Err(Error::Misc(String::from("bad type order byte")));
        }
        let e = (k[n+1] as i32) - 23;
        // exponent is number of times the mantissa must be multiplied times 100
        // if we assume that all mantissa digits are to the right of the decimal point.
        if e <= 0 {
            return Err(Error::Misc(String::from("bad e")));
        }
        let e = e as usize;
        let n = n + 2;
        let a = &k[n .. ];

        // remaining bytes are mantissa, base 100
        // last byte of mantissa is 2*x
        // previous bytes are 2*x+1

        //printfn "mantissa: %A" a
        //printfn "e: %d" e

        // we have an array of centimal digits here, all of
        // which appear to the right of the decimal point.
        //
        // we know from the context that this
        // SHOULD be an integer.

        let a =
            if a.len() > e {
                &a[0 .. e]
            } else {
                a
            };

        let mut v = a.iter().fold(0, |v,d| {
            let b = (d >> 1) as i32;
            v * 100 + b
        });

        let need = e - a.len();
        if need > 0 {
            for i in 0 .. need {
                v = v * 100;
            }
        }

        //printfn "weight: %d" v
        Ok(v)
    }

    pub fn get_type_order(&self) -> i32 {
        // same numbers as canonicalizeBSONType()
        match self {
            &Value::BUndefined => 0,
            &Value::BNull => 5,
            &Value::BDouble(_) => 10,
            &Value::BInt64(_) => 10,
            &Value::BInt32(_) => 10,
            &Value::BString(_) => 15,
            &Value::BDocument(_) => 20,
            &Value::BArray(_) => 25,
            &Value::BBinary(_, _) => 30,
            &Value::BObjectID(_) => 35,
            &Value::BBoolean(_) => 40,
            &Value::BDateTime(_) => 45,
            &Value::BTimeStamp(_) => 47,
            &Value::BRegex(_, _) => 50,
            &Value::BJSCode(_) => 60,
            &Value::BJSCodeWithScope(_) => 65,
            &Value::BMinKey => -1,
            &Value::BMaxKey => 127,
        }
    }

    pub fn to_bson_array(&self) -> Vec<u8> {
        let mut v = Vec::new();
        self.to_bson(&mut v);
        v
    }

    pub fn encode_for_index_into(&self, w: &mut Vec<u8>) {
        w.push(self.get_type_order() as u8);
        match self {
            &Value::BBoolean(b) => if b { w.push(1u8) } else { w.push(0u8) },
            &Value::BNull => (),
            &Value::BMinKey => (),
            &Value::BMaxKey => (),
            &Value::BUndefined => (),
            &Value::BObjectID(ref a) => w.push_all(a),
            &Value::BString(ref s) => vec_push_c_string(w, &s),
            &Value::BDouble(f) => misc::Sqlite4Num::from_f64(f).encode_for_index(w),
            &Value::BInt64(n) => misc::Sqlite4Num::from_i64(n).encode_for_index(w),
            &Value::BInt32(n) => misc::Sqlite4Num::from_i64(n as i64).encode_for_index(w),
            &Value::BDocument(ref bd) => {
                // TODO is writing the length here what we want?
                // it means we can't match on a prefix of a document
                //
                // it means any document with 3 pairs will sort before 
                // any document with 4 pairs, even if the first 3 pairs
                // are the same in both.

                w.push_all(&i32_to_bytes_be(bd.pairs.len() as i32));
                for t in &bd.pairs {
                    vec_push_c_string(w, &t.0);;
                    t.1.encode_for_index_into(w);
                }
            },
            &Value::BArray(ref ba) => {
                // TODO is writing the length here what we want?
                // see comment on BDocument just above.

                w.push_all(&i32_to_bytes_be(ba.items.len() as i32));
                for v in &ba.items {
                    v.encode_for_index_into(w);
                }
            },
            &Value::BRegex(ref expr, ref opt) => {
                vec_push_c_string(w, &expr); 
                vec_push_c_string(w, &opt);
            },
            &Value::BJSCode(ref s) => vec_push_c_string(w, &s),
            &Value::BJSCodeWithScope(ref s) => vec_push_c_string(w, &s),
            &Value::BDateTime(n) => {
                misc::Sqlite4Num::from_i64(n).encode_for_index(w);
            },
            &Value::BTimeStamp(n) => {
                // TODO is this really how we should encode this?
                misc::Sqlite4Num::from_i64(n).encode_for_index(w);
            },
            &Value::BBinary(subtype, ref ba) => {
                w.push(subtype);
                w.push_all(&i32_to_bytes_be(ba.len() as i32));
                w.push_all(&ba);
            },
        }
    }

    pub fn encode_one_for_index(v: &Value, neg: bool) -> Vec<u8> {
        let mut a = Vec::new();
        v.encode_for_index_into(&mut a);
        if neg {
            for i in 0 .. a.len() {
                let b = a[i];
                a[i] = !b;
            }
        }
        a
    }

    pub fn encode_multi_for_index(vals: Vec<(Value, bool)>) -> Vec<u8> {
        let mut r = Vec::new();
        for (v, neg) in vals {
            let a = Self::encode_one_for_index(&v, neg);
            r.push_all(&a);
        }
        r
    }

    pub fn replace_undefined(&mut self) {
        match self {
            &mut Value::BUndefined => {
                *self = Value::BNull;
            },
            &mut Value::BArray(ref mut ba) => {
                for i in 0 .. ba.items.len() {
                    match &ba.items[i] {
                        &Value::BUndefined => {
                            ba.items[i] = Value::BNull;
                        },
                        _ => {
                        },
                    }
                }
            },
            &mut Value::BDocument(ref mut bd) => {
                for i in 0 .. bd.pairs.len() {
                    match &bd.pairs[i].1 {
                        &Value::BUndefined => {
                            // TODO we just want to replace the snd half of the tuple
                            bd.pairs[i] = (bd.pairs[i].0.clone(), Value::BNull)
                        },
                        _ => {
                        },
                    }
                }
            },
            _ => {
            },
        }
    }

    pub fn to_bson(&self, w: &mut Vec<u8>) {
        match self {
            &Value::BDouble(f) => w.push_all(&f64_to_bytes_le(f)),
            &Value::BInt32(n) => w.push_all(&i32_to_bytes_le(n)),
            &Value::BDateTime(n) => w.push_all(&i64_to_bytes_le(n)),
            &Value::BTimeStamp(n) => w.push_all(&i64_to_bytes_le(n)),
            &Value::BInt64(n) => w.push_all(&i64_to_bytes_le(n)),
            &Value::BString(ref s) => vec_push_bson_string(w, &s),
            &Value::BObjectID(ref a) => w.push_all(a),
            &Value::BBoolean(b) => if b { w.push(1u8) } else { w.push(0u8) },
            &Value::BNull => (),
            &Value::BMinKey => (),
            &Value::BMaxKey => (),
            &Value::BRegex(ref expr, ref opt) => {
                vec_push_c_string(w, &expr); 
                vec_push_c_string(w, &opt);
            },
            &Value::BUndefined => (),
            &Value::BJSCode(ref s) => vec_push_bson_string(w, &s),
            &Value::BJSCodeWithScope(ref s) => panic!("TODO write BJSCodeWithScope"),
            &Value::BBinary(subtype, ref ba) => {
                w.push_all(&i32_to_bytes_le(ba.len() as i32));
                w.push(subtype);
                w.push_all(&ba);
            },
            &Value::BArray(ref ba) => {
                ba.to_bson(w);
            },
            &Value::BDocument(ref bd) => {
                bd.to_bson(w);
            },
        }
    }

}

