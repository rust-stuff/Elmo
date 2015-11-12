/*
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

#![feature(convert)]
#![feature(iter_arith)]

use std::collections::BTreeMap;

extern crate lsm;
use lsm::ICursor;
use lsm::IForwardCursor;

extern crate rand;
use rand::Rng;
use rand::SeedableRng;

fn dump_page(name: &str, pgnum: u32) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let page = try!(db.get_page(pgnum));
    println!("{:?}", page);
    Ok(())
}

fn show_page(name: &str, pgnum: u32) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let cursor = try!(db.open_cursor_on_page(pgnum));
    let pt = cursor.page_type();
    println!("page type: {:?}", pt);
    // TODO
    Ok(())
}

fn merge(name: &str, from_level: String) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let from_level =
        match from_level.as_str() {
            "fresh" => {
                lsm::FromLevel::Fresh
            },
            "young" => {
                lsm::FromLevel::Young
            },
            _ => {
                let level = from_level.parse::<usize>().unwrap();
                lsm::FromLevel::Other(level)
            },
        };
    try!(db.merge(from_level));
    Ok(())
}

fn show_leaf_page(name: &str, pgnum: u32) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let mut cursor = try!(db.open_cursor_on_leaf_page(pgnum));
    try!(cursor.First());
    while cursor.IsValid() {
        {
            let k = try!(cursor.KeyRef());
            println!("k: {:?}", k);
            let v = try!(cursor.ValueRef());
            //println!("v: {:?}", v);
            //let q = try!(v.into_boxed_slice());
        }
        try!(cursor.Next());
    }
    Ok(())
}

fn show_parent_page(name: &str, pgnum: u32) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let page = try!(db.read_parent_page(pgnum));
    println!("depth: {}", page.depth());
    println!("count_items: {}", page.count_items());
    let blocks = try!(page.blocklist_clean());
    println!("blocks ({} blocks, {} pages): {:?}", blocks.count_blocks(), blocks.count_pages(), blocks);
    println!("key range: {:?}", try!(page.range()));
    let count = page.count_items();
    println!("items ({}):", count);
    for i in 0 .. count {
        let p = page.get_child_pagenum(i);
        println!("    {}", p);
        let child_range = try!(page.child_range(i));
        println!("    {:?}", child_range);
    }
    try!(page.verify_child_keys());
    Ok(())
}

fn list_segments(name: &str) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let (fresh, young, levels) = try!(db.list_segments());
    println!("fresh ({}): ", fresh.len());

    fn print_seg(s: &lsm::SegmentLocation) {
        println!("    {}, {} pages", s.root_page, 1 + s.blocks.count_pages());
    }

    for s in fresh.iter() {
        print_seg(s);
    }
    println!("young ({}): ", young.len());
    for s in young.iter() {
        print_seg(s);
    }
    println!("levels ({}): ", levels.len());
    for s in levels.iter() {
        print_seg(s);
    }
    Ok(())
}

fn list_free_blocks(name: &str) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let blocks = try!(db.list_free_blocks());
    println!("{:?}", blocks);
    println!("total pages: {}", blocks.count_pages());
    Ok(())
}

fn list_keys(name: &str) -> Result<(),lsm::Error> {
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let mut cursor = try!(db.open_cursor());
    try!(cursor.First());
    while cursor.IsValid() {
        {
            let k = try!(cursor.KeyRef());
            println!("k: {:?}", k);
            let v = try!(cursor.LiveValueRef());
            //println!("v: {:?}", v);
            //let q = try!(v.into_boxed_slice());
        }
        try!(cursor.Next());
    }
    Ok(())
}

fn seek_string(name: &str, key: String, sop: String) -> Result<(),lsm::Error> {
    let sop = 
        match sop.as_str() {
            "eq" => lsm::SeekOp::SEEK_EQ,
            "le" => lsm::SeekOp::SEEK_LE,
            "ge" => lsm::SeekOp::SEEK_GE,
            _ => return Err(lsm::Error::Misc(String::from("invalid sop"))),
        };
    let k = key.into_bytes().into_boxed_slice();
    let k = lsm::KeyRef::from_boxed_slice(k);
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let mut cursor = try!(db.open_cursor());
    let sr = try!(cursor.SeekRef(&k, sop));
    println!("sr: {:?}", sr);
    if cursor.IsValid() {
        {
            let k = try!(cursor.KeyRef());
            println!("k: {:?}", k);
            let v = try!(cursor.LiveValueRef());
            println!("v: {:?}", v);
        }
    }
    Ok(())
}

fn seek_bytes(name: &str, k: Box<[u8]>, sop: String) -> Result<(),lsm::Error> {
    let sop = 
        match sop.as_str() {
            "eq" => lsm::SeekOp::SEEK_EQ,
            "le" => lsm::SeekOp::SEEK_LE,
            "ge" => lsm::SeekOp::SEEK_GE,
            _ => return Err(lsm::Error::Misc(String::from("invalid sop"))),
        };
    let k = lsm::KeyRef::from_boxed_slice(k);
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let mut cursor = try!(db.open_cursor());
    let sr = try!(cursor.SeekRef(&k, sop));
    println!("RESULT sr: {:?}", sr);
    if cursor.IsValid() {
        {
            let k = try!(cursor.KeyRef());
            println!("k: {:?}", k);
            let v = try!(cursor.LiveValueRef());
            println!("v: {:?}", v);
        }
        for x in 0 .. 20 {
            try!(cursor.Next());
            if cursor.IsValid() {
                let k = try!(cursor.KeyRef());
                println!("    k: {:?}", k);
                let v = try!(cursor.LiveValueRef());
                println!("    v: {:?}", v);
            } else {
                break;
            }
        }
    }
    Ok(())
}

fn add_numbers(name: &str, count: u64, start: u64, step: u64) -> Result<(),lsm::Error> {
    let mut pending = BTreeMap::new();
    for i in 0 .. count {
        let val = start + i * step;
        let k = format!("{:08}", val).into_bytes().into_boxed_slice();
        let v = format!("{}", val).into_bytes().into_boxed_slice();
        pending.insert(k, lsm::Blob::Boxed(v));
    }
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let seg = try!(db.write_segment(pending).map_err(lsm::wrap_err));
    if let Some(seg) = seg {
        let lck = try!(db.get_write_lock());
        try!(lck.commit_segment(seg).map_err(lsm::wrap_err));
    }
    Ok(())
}

fn add_random(name: &str, count: u64, seed: usize, klen: usize, vlen: usize) -> Result<(),lsm::Error> {
    fn make(rng: &mut rand::StdRng, max_len: usize) -> Box<[u8]> {
        let len = (rng.next_u64() as usize) % max_len + 1;
        let mut k = vec![0u8; len].into_boxed_slice();
        rng.fill_bytes(&mut k);
        k
    }

    let mut rng = rand::StdRng::from_seed(&[seed]);
    let mut pending = BTreeMap::new();
    for i in 0 .. count {
        let k = make(&mut rng, klen);
        let v = make(&mut rng, vlen);
        pending.insert(k, lsm::Blob::Boxed(v));
    }
    let db = try!(lsm::DatabaseFile::new(String::from(name), lsm::SETTINGS_NO_AUTOMERGE));
    let seg = try!(db.write_segment(pending).map_err(lsm::wrap_err));
    if let Some(seg) = seg {
        let lck = try!(db.get_write_lock());
        try!(lck.commit_segment(seg).map_err(lsm::wrap_err));
    }
    Ok(())
}

fn result_main() -> Result<(),lsm::Error> {
    let args: Vec<_> = std::env::args().collect();
    println!("args: {:?}", args);
    if args.len() < 2 {
        return Err(lsm::Error::Misc(String::from("no filename given")));
    }
    if args.len() < 3 {
        return Err(lsm::Error::Misc(String::from("no command given")));
    }
    let name = args[1].as_str();
    let cmd = args[2].as_str();
    match cmd {
        "add_random" => {
            println!("usage: add_random count seed klen vlen");
            if args.len() < 7 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let count = args[3].parse::<u64>().unwrap();
            let seed = args[4].parse::<usize>().unwrap();
            let klen = args[5].parse::<usize>().unwrap();
            let vlen = args[6].parse::<usize>().unwrap();
            add_random(name, count, seed, klen, vlen)
        },
        "add_numbers" => {
            println!("usage: add_numbers count start step");
            if args.len() < 6 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let count = args[3].parse::<u64>().unwrap();
            let start = args[4].parse::<u64>().unwrap();
            let step = args[5].parse::<u64>().unwrap();
            if step == 0 {
                return Err(lsm::Error::Misc(String::from("step cannot be 0")));
            }
            add_numbers(name, count, start, step)
        },
        "show_page" => {
            println!("usage: show_page pagenum");
            if args.len() < 4 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let pgnum = args[3].parse::<u32>().unwrap();
            show_page(name, pgnum)
        },
        "show_leaf_page" => {
            println!("usage: show_leaf_page pagenum");
            if args.len() < 4 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let pgnum = args[3].parse::<u32>().unwrap();
            show_leaf_page(name, pgnum)
        },
        "show_parent_page" => {
            println!("usage: show_parent_page pagenum");
            if args.len() < 4 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let pgnum = args[3].parse::<u32>().unwrap();
            show_parent_page(name, pgnum)
        },
        "seek_string" => {
            println!("usage: seek_string key sop");
            if args.len() < 5 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let key = args[3].clone();
            let sop = args[4].clone();
            seek_string(name, key, sop)
        },
        "merge" => {
            println!("usage: merge from_level");
            if args.len() < 4 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let from_level = args[3].clone();
            merge(name, from_level)
        },
        "seek_bytes" => {
            println!("usage: seek_bytes sop numbytes b1 b2 b3 ... ");
            if args.len() < 5 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let sop = args[3].clone();
            let count = args[4].parse::<usize>().unwrap();
            if count == 0 {
                return Err(lsm::Error::Misc(String::from("count cannot be 0")));
            }
            if args.len() < 5 + count {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let mut k = Vec::with_capacity(count);
            for i in 0 .. count {
                let b = args[5 + i].parse::<u8>().unwrap();
                k.push(b);
            }
            seek_bytes(name, k.into_boxed_slice(), sop)
        },
        "dump_page" => {
            if args.len() < 4 {
                return Err(lsm::Error::Misc(String::from("too few args")));
            }
            let pgnum = args[3].parse::<u32>().unwrap();
            dump_page(name, pgnum)
        },
        "list_keys" => {
            list_keys(name)
        },
        "list_segments" => {
            list_segments(name)
        },
        "list_free_blocks" => {
            list_free_blocks(name)
        },
        _ => {
            Err(lsm::Error::Misc(String::from("unknown command")))
        },
    }
}

pub fn main() {
    let r = result_main();
    if r.is_err() {
        println!("{:?}", r);
    }
}

