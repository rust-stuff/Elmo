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

// This server exists so that we can run the jstests suite (from the MongoDB
// source repo on GitHub) against Elmo.  It is *not* expected that an actual
// server listening on a socket would be useful for the common use cases on 
// a mobile device.

#![feature(box_syntax)]
#![feature(convert)]
#![feature(associated_consts)]
#![feature(vec_push_all)]

extern crate misc;

use misc::endian;
use misc::bufndx;

extern crate bson;

extern crate elmo;

extern crate elmo_lsm;

use std::io::Read;

use elmo::Error;
use elmo::Result;

#[derive(Debug)]
struct Reply {
    req_id : i32,
    response_to : i32,
    flags : i32,
    cursor_id : i64,
    starting_from : i32,
    docs : Vec<bson::Document>,
}

#[derive(Debug)]
// TODO consider calling this Msg2004
struct MsgQuery {
    req_id : i32,
    flags : i32,
    full_collection_name : String,
    number_to_skip : i32,
    number_to_return : i32,
    query : bson::Document,
    return_fields_selector : Option<bson::Document>,
}

#[derive(Debug)]
struct MsgGetMore {
    req_id : i32,
    full_collection_name : String,
    number_to_return : i32,
    cursor_id : i64,
}

#[derive(Debug)]
struct MsgKillCursors {
    req_id : i32,
    cursor_ids : Vec<i64>,
}

#[derive(Debug)]
enum Request {
    Query(MsgQuery),
    GetMore(MsgGetMore),
    KillCursors(MsgKillCursors),
}

impl Reply {
    fn encode(&self) -> Box<[u8]> {
        let mut w = Vec::new();
        // length placeholder
        w.push_all(&[0u8; 4]);
        w.push_all(&endian::i32_to_bytes_le(self.req_id));
        w.push_all(&endian::i32_to_bytes_le(self.response_to));
        w.push_all(&endian::u32_to_bytes_le(1u32)); 
        w.push_all(&endian::i32_to_bytes_le(self.flags));
        w.push_all(&endian::i64_to_bytes_le(self.cursor_id));
        w.push_all(&endian::i32_to_bytes_le(self.starting_from));
        w.push_all(&endian::u32_to_bytes_le(self.docs.len() as u32));
        for doc in &self.docs {
            doc.to_bson(&mut w);
        }
        misc::bytes::copy_into(&endian::u32_to_bytes_le(w.len() as u32), &mut w[0 .. 4]);
        w.into_boxed_slice()
    }
}

fn vec_rows_to_values(v: Vec<elmo::Row>) -> Vec<bson::Value> {
    v.into_iter().map(|r| r.doc).collect::<Vec<_>>()
}

fn vec_values_to_docs(v: Vec<bson::Value>) -> Result<Vec<bson::Document>> {
    let a = try!(v.into_iter().map(|d| d.into_document()).collect::<std::result::Result<Vec<_>, bson::Error>>());
    Ok(a)
}

fn vec_docs_to_values(v: Vec<bson::Document>) -> Vec<bson::Value> {
    v.into_iter().map(|d| bson::Value::BDocument(d)).collect::<Vec<_>>()
}

fn parse_request(ba: &[u8]) -> Result<Request> {
    let mut i = 0;
    let (message_len,req_id,response_to,op_code) = slurp_header(ba, &mut i);
    match op_code {
        2004 => {
            let flags = bufndx::slurp_i32_le(ba, &mut i);
            let full_collection_name = try!(bufndx::slurp_cstring(ba, &mut i));
            let number_to_skip = bufndx::slurp_i32_le(ba, &mut i);
            let number_to_return = bufndx::slurp_i32_le(ba, &mut i);
            let query = try!(bson::slurp_document(ba, &mut i));
            let return_fields_selector = if i < ba.len() { Some(try!(bson::slurp_document(ba, &mut i))) } else { None };

            let msg = MsgQuery {
                req_id: req_id,
                flags: flags,
                full_collection_name: full_collection_name,
                number_to_skip: number_to_skip,
                number_to_return: number_to_return,
                query: query,
                return_fields_selector: return_fields_selector,
            };
            Ok(Request::Query(msg))
        },

        2005 => {
            let flags = bufndx::slurp_i32_le(ba, &mut i);
            let full_collection_name = try!(bufndx::slurp_cstring(ba, &mut i));
            let number_to_return = bufndx::slurp_i32_le(ba, &mut i);
            let cursor_id = bufndx::slurp_i64_le(ba, &mut i);

            let msg = MsgGetMore {
                req_id: req_id,
                full_collection_name: full_collection_name,
                number_to_return: number_to_return,
                cursor_id: cursor_id,
            };
            Ok(Request::GetMore(msg))
        },

        2007 => {
            let flags = bufndx::slurp_i32_le(ba, &mut i);
            let number_of_cursor_ids = bufndx::slurp_i32_le(ba, &mut i);
            let mut cursor_ids = Vec::new();
            for _ in 0 .. number_of_cursor_ids {
                cursor_ids.push(bufndx::slurp_i64_le(ba, &mut i));
            }

            let msg = MsgKillCursors {
                req_id: req_id,
                cursor_ids: cursor_ids,
            };
            Ok(Request::KillCursors(msg))
        },

        _ => {
            Err(Error::CorruptFile("unknown message opcode TODO"))
        },
    }
}

// TODO do these really need to be signed?
fn slurp_header(ba: &[u8], i: &mut usize) -> (i32,i32,i32,i32) {
    let message_len = bufndx::slurp_i32_le(ba, i);
    let req_id = bufndx::slurp_i32_le(ba, i);
    let response_to = bufndx::slurp_i32_le(ba, i);
    let op_code = bufndx::slurp_i32_le(ba, i);
    let v = (message_len, req_id, response_to, op_code);
    v
}

fn read_message_bytes<R: Read>(stream: &mut R) -> Result<Option<Box<[u8]>>> {
    let mut a = [0; 4];
    let got = try!(misc::io::read_fully(stream, &mut a));
    if got == 0 {
        return Ok(None);
    }
    let message_len = endian::u32_from_bytes_le(a) as usize;
    let mut msg = vec![0; message_len]; 
    misc::bytes::copy_into(&a, &mut msg[0 .. 4]);
    let got = try!(misc::io::read_fully(stream, &mut msg[4 .. message_len]));
    if got != message_len - 4 {
        return Err(Error::CorruptFile("end of file at the wrong time"));
    }
    Ok(Some(msg.into_boxed_slice()))
}

fn create_reply(req_id: i32, docs: Vec<bson::Document>, cursor_id: i64) -> Reply {
    let msg = Reply {
        req_id: 0,
        response_to: req_id,
        flags: 0,
        cursor_id: cursor_id,
        starting_from: 0,
        // TODO
        docs: docs,
    };
    msg
}

fn reply_err(req_id: i32, err: Error) -> Reply {
    let mut doc = bson::Document::new();
    doc.set_string("$err", format!("{:?}", err));
    match err {
        elmo::Error::MongoCode(code, _) => {
            doc.set_i32("code", code);
        },
        _ => {
        },
    }
    doc.set_i32("ok", 0);
    let mut r = create_reply(req_id, vec![doc], 0);
    r.flags = 2;
    r
}

fn reply_errmsg(req_id: i32, err: Error) -> Reply {
    let mut doc = bson::Document::new();
    doc.set_string("errmsg", format!("{:?}", err));
    match err {
        elmo::Error::MongoCode(code, _) => {
            doc.set_i32("code", code);
        },
        _ => {
        },
    }
    doc.set_i32("ok", 0);
    create_reply(req_id, vec![doc], 0)
}

// TODO mongo has a way of automatically killing a cursor after 10 minutes idle

struct Server<'a> {
    factory: Box<elmo::ConnectionFactory>,
    cursor_num: i64,
    conn: elmo::Connection,
    // TODO this is problematic when/if the Iterator has a reference to or the same lifetime
    // as self.conn.
    cursors: std::collections::HashMap<i64, (String, elmo::Connection, Box<Iterator<Item=Result<elmo::Row>> + 'a>)>,
}

impl<'b> Server<'b> {

    pub fn new(factory: Box<elmo::ConnectionFactory>) -> Server<'b> {
        let conn = factory.open().expect("TODO");
        Server {
            factory: factory,
            conn: conn,
            cursor_num: 0,
            cursors: std::collections::HashMap::new(),
        }
    }

    fn reply_whatsmyuri(&self, req: &MsgQuery) -> Result<Reply> {
        //println!("----------------------------------------------------------------");
        //println!("----------------------------------------------------------------");
        let mut doc = bson::Document::new();
        doc.set_str("you", "127.0.0.1:65460");
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_getlog(&self, req: &MsgQuery) -> Result<Reply> {
        let mut doc = bson::Document::new();
        doc.set_i32("totalLinesWritten", 1);
        doc.set_array("log", bson::Array::new());
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_replsetgetstatus(&self, req: &MsgQuery) -> Result<Reply> {
        let mut mine = bson::Document::new();
        mine.set_i32("_id", 0);
        mine.set_str("name", "whatever");
        mine.set_i32("state", 1);
        mine.set_f64("health", 1.0);
        mine.set_str("stateStr", "PRIMARY");
        mine.set_i32("uptime", 0);
        mine.set_timestamp("optime", 0);
        mine.set_datetime("optimeDate", 0);
        mine.set_timestamp("electionTime", 0);
        mine.set_timestamp("electionDate", 0);
        mine.set_bool("self", true);

        let mut doc = bson::Document::new();
        doc.set_document("mine", mine);
        doc.set_str("set", "TODO");
        doc.set_datetime("date", 0);
        doc.set_i32("myState", 1);
        doc.set_i32("ok", 1);

        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_rename_collection(&self, req: &MsgQuery) -> Result<Reply> {
        let old_name = try!(req.query.must_get_str("renameCollection"));
        let new_name = try!(req.query.must_get_str("to"));
        let drop_target = 
            match req.query.get("dropTarget") {
                Some(v) => {
                    match v {
                        &bson::Value::BBoolean(b) => b,
                        _ => false,
                    }
                },
                None => {
                    false
                },
            };
        let result = try!(self.conn.rename_collection(old_name, new_name, drop_target));
        let mut doc = bson::Document::new();
        // TODO shouldn't result get used or sent back here?
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_buildinfo(&self, req: &MsgQuery) -> Result<Reply> {
        Err(Error::Misc(format!("TODO buildinfo: {:?}", req)))
    }

    fn reply_serverstatus(&self, req: &MsgQuery) -> Result<Reply> {
        Err(Error::Misc(format!("TODO serverstatus: {:?}", req)))
    }

    fn reply_setparameter(&self, req: &MsgQuery) -> Result<Reply> {
        // TODO what is this for?
        let mut doc = bson::Document::new();
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_ismaster(&self, req: &MsgQuery) -> Result<Reply> {
        let mut doc = bson::Document::new();
        // TODO setName
        doc.set_str("setName", "foo");
        doc.set_i32("setVersion", 1);
        doc.set_bool("ismaster", true);
        doc.set_bool("secondary", false);
        doc.set_i32("maxWireVersion", 3);
        doc.set_i32("minWireVersion", 2);
        // ver >= 2:  we don't support the older fire-and-forget write operations. 
        // ver >= 3:  we don't support the older form of explain
        // TODO if we set minWireVersion to 3, which is what we want to do, so
        // that we can tell the client that we don't support the older form of
        // explain, what happens is that we start getting the old fire-and-forget
        // write operations instead of the write commands that we want.
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_cmd_sys_inprog(&self, req: &MsgQuery, db: &str) -> Result<Reply> {
        let mut doc = bson::Document::new();
        doc.set_array("inprog", bson::Array::new());
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_admin_cmd(&self, req: &MsgQuery, db: &str) -> Result<Reply> {
        use std::ascii::AsciiExt;
        if req.query.pairs.is_empty() {
            Err(Error::Misc(String::from("empty query")))
        } else {
            // this code assumes that the first key is always the command
            let cmd = req.query.pairs[0].0.clone().to_ascii_lowercase();
            let res =
                match cmd.as_str() {
                    "whatsmyuri" => self.reply_whatsmyuri(req),
                    "getlog" => self.reply_getlog(req),
                    "replsetgetstatus" => self.reply_replsetgetstatus(req),
                    "ismaster" => self.reply_ismaster(req),
                    "renamecollection" => self.reply_rename_collection(req),
                    "buildinfo" => self.reply_buildinfo(req),
                    "serverstatus" => self.reply_serverstatus(req),
                    "setparameter" => self.reply_setparameter(req),
                    _ => Err(Error::Misc(format!("unknown admin cmd: {}", cmd)))
                };
            res
        }
    }

    fn reply_delete(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("delete"));
        let deletes = try!(req.query.must_remove_array("deletes"));
        let deletes = try!(vec_values_to_docs(deletes.items));
        // TODO limit
        // TODO ordered
        let result = try!(self.conn.delete(db, &coll, deletes));
        let mut doc = bson::Document::new();
        doc.set_i32("n", result as i32);
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_update(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("update"));
        let updates = try!(req.query.must_remove_array("updates"));
        let mut updates = try!(vec_values_to_docs(updates.items));
        // TODO ordered
        // TODO do we need to keep ownership of updates?
        let results = try!(self.conn.update(db, &coll, &mut updates, &*self.factory));
        let mut matches = 0;
        let mut mods = 0;
        let mut upserts = bson::Array::new();
        let mut errors = bson::Array::new();
        for (i,r) in results.into_iter().enumerate() {
            match r {
                Ok((count_matched, count_modified, upserted)) => {
                    matches = matches + count_matched;
                    mods = mods + count_modified;
                    match upserted{
                        Some(id) => {
                            let mut doc = bson::Document::new();
                            doc.set_i32("index", i as i32);
                            doc.set("_id", id);
                            upserts.push(doc.into_value());
                        },
                        None => {
                        },
                    }
                },
                Err(e) => {
                    let mut doc = bson::Document::new();
                    doc.set_i32("index", i as i32);
                    doc.set_string("errmsg", format!("{}", e));
                    errors.push(doc.into_value());
                },
            }
        }
        let mut doc = bson::Document::new();
        doc.set_i32("n", matches + (upserts.len() as i32));
        doc.set_i32("modified", mods);
        if upserts.len() > 0 {
            doc.set_array("upserted", upserts);
        }
        if errors.len() > 0 {
            doc.set_array("writeErrors", errors);
        }
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_profile(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        Err(Error::Misc(format!("TODO profile: {:?}", req)))
    }

    fn reply_collstats(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        Err(Error::Misc(format!("TODO collstats: {:?}", req)))
    }

    fn reply_distinct(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("distinct"));
        let key = try!(req.query.must_remove("key"));
        let key = try!(key.into_string().map_err(|e| elmo::Error::MongoCode(18510, format!("must be string"))));
        let query = try!(req.query.must_remove("query"));
        let query = try!(query.into_document().map_err(|e| elmo::Error::MongoCode(18511, format!("must be document"))));

        let values = try!(self.conn.distinct(db, &coll, &key, query));
        let mut doc = bson::Document::new();
        doc.set_array("values", values);
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_explain(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;
        let mut explain = try!(query.must_remove_document("explain"));
        let verbosity = try!(query.must_remove_string("verbosity"));

        let coll = try!(explain.must_remove_string("find"));
        let filter = try!(explain.must_remove_document("filter"));
        let mut options = try!(explain.must_remove_document("options"));

        let orderby = Self::try_remove_optional_prefix(&mut options, "$orderby");
        let min = Self::try_remove_optional_prefix(&mut options, "$min");
        let max = Self::try_remove_optional_prefix(&mut options, "$max");
        let hint = Self::try_remove_optional_prefix(&mut options, "$hint");
        let explain = Self::try_remove_optional_prefix(&mut options, "$explain");

        let mut seq = try!(self.conn.find(
                db, 
                &coll, 
                filter,
                orderby,
                return_fields_selector,
                min,
                max,
                hint,
                None
                ));

        if number_to_skip < 0 {
            return Err(Error::Misc(format!("negative skip: {}", number_to_skip)));
        } else if number_to_skip > 0 {
            seq = box seq.skip(number_to_skip as usize);
        }

        let num = seq.count();

        /*

            "queryPlanner" : {
                "plannerVersion" : 1,
                "namespace" : "test.foo",
                "indexFilterSet" : false,
                "parsedQuery" : {
                    "a" : {
                        "$eq" : 1
                    }
                },
                "winningPlan" : {
                    "stage" : "FETCH",
                    "inputStage" : {
                        "stage" : "IXSCAN",
                        "keyPattern" : {
                            "a" : 1
                        },
                        "indexName" : "a_1",
                        "isMultiKey" : true,
                        "direction" : "forward",
                        "indexBounds" : {
                            "a" : [
                                "[1.0, 1.0]"
                            ]
                        }
                    }
                },
                "rejectedPlans" : [ ]
            },
            "serverInfo" : {
                "host" : "erics-air-2.ad.sourcegear.com",
                "port" : 27017,
                "version" : "3.0.1",
                "gitVersion" : "534b5a3f9d10f00cd27737fbcd951032248b5952"
            },

        */

        let mut doc = bson::Document::new();
        try!(doc.set_path("queryPlanner.namespace", bson::Value::BString(format!("{}.{}", db, &coll))));
        try!(doc.set_path("queryPlanner.indexFilterSet", bson::Value::BBoolean(false)));

        try!(doc.set_path("serverInfo.software", bson::Value::BString(format!("Elmo"))));
        try!(doc.set_path("serverInfo.version", bson::Value::BString(format!("TODO"))));
        try!(doc.set_path("serverInfo.host", bson::Value::BString(format!("TODO"))));
        // TODO port
        try!(doc.set_path("serverInfo.port", bson::Value::BString(format!("27017"))));

        try!(doc.set_path("executionStats.nReturned", bson::Value::BInt32(num as i32)));
        // TODO fake values below
        try!(doc.set_path("executionStats.totalKeysExamined", bson::Value::BInt32(0)));
        try!(doc.set_path("executionStats.totalDocsExamined", bson::Value::BInt32(0)));

        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_find_and_modify(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;
        let coll = try!(query.must_removenocase_string("findandmodify"));
        let filter = query.remove("query");
        let sort = query.remove("sort");
        let remove = query.remove("remove");
        let update = query.remove("update");
        let new = query.remove("new");
        let new = match new {
            Some(bson::Value::BBoolean(b)) => b,
            //Some(_) => TODO error?
            _ => false,
        };
        let fields = query.remove("fields");
        let upsert = query.remove("upsert");
        let upsert = match upsert {
            Some(bson::Value::BBoolean(b)) => b,
            //Some(_) => TODO error?
            _ => false,
        };

        let was_update = update.is_some();
        let t = try!(self.conn.find_and_modify(db, &coll, filter, sort, remove, update, new, upsert));
        let (found,err,changed,upserted,result) = t;

        let mut last_error_object = bson::Document::new();
        let mut doc = bson::Document::new();

        // TODO docs say: The updatedExisting field only appears if the command specifies an update or an update 
        // with upsert: true; i.e. the field does not appear for a remove.

        // TODO always 1 ?  appears for every op?
        last_error_object.set_i32("n", if changed {1} else {0});

        match upserted {
            Some(id) => {
                last_error_object.set("upserted", id);
            },
            _ => (),
        }

        match (was_update,found,upsert) {
            (true,false,_) => last_error_object.set_bool("updatedExisting", false),
            (true,true,false) => last_error_object.set_bool("updatedExisting", changed),
            _ => last_error_object.set_bool("updatedExisting", changed),
        }

        // TODO docs say: if not found, for update or remove (but not upsert), then
        // lastErrorObject does not appear in the return document and the value field holds a null.

        // TODO docs say: for update with upsert: true operation that results in an insertion, if the command 
        // also specifies new is false and specifies a sort, the return document has a lastErrorObject, value, 
        // and ok fields, but the value field holds an empty document {}.

        // TODO docs say: for update with upsert: true operation that results in an insertion, if the command 
        // also specifies new is false but does not specify a sort, the return document has a lastErrorObject, value, 
        // and ok fields, but the value field holds a null.

        match err {
            Some(e) => {
                // TODO is this right?  docs don't say this.
                last_error_object.set_string("errmsg", format!("{}", e));
                doc.set_i32("ok", 0);
            },
            None => {
                doc.set_i32("ok", 1);
            },
        }

        match result {
            Some(v) => {
                match fields {
                    Some(proj) => {
                        // TODO calling stuff below that seems like it should be private to the crud module
                        let projection = try!(elmo::Projection::parse(try!(proj.into_document())));
                        // TODO projection position op allowed here?
                        let row = elmo::Row {
                            doc: v.into_value(),
                            pos: None,
                            score: None,
                        };
                        let row = try!(projection.project(row));
                        doc.set("value", row.doc);
                    },
                    None => {
                        doc.set_document("value", v);
                    },
                }
            },
            None => {
                doc.set("value", bson::Value::BNull);
            },
        }

        doc.set_document("lastErrorObject", last_error_object);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_insert(&self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("insert"));

        let docs = try!(req.query.must_remove_array("documents"));
        let mut docs = try!(vec_values_to_docs(docs.items));

        // TODO ordered
        // TODO do we need to keep ownership of docs?
        let results = try!(self.conn.insert(db, &coll, &mut docs));
        let mut errors = Vec::new();
        for i in 0 .. results.len() {
            if results[i].is_err() {
                let msg = format!("{:?}", results[i]);
                let err = bson::Value::BDocument(bson::Document {pairs: vec![(String::from("index"), bson::Value::BInt32(i as i32)), (String::from("errmsg"), bson::Value::BString(msg))]});
                errors.push(err);
            }
        }
        let mut doc = bson::Document::new();
        doc.set_i32("n", ((results.len() - errors.len()) as i32));
        if errors.len() > 0 {
            doc.set_array("writeErrors", bson::Array {items: errors});
        }
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn store_cursor<T: Iterator<Item=Result<elmo::Row>> + 'b>(&mut self, ns: &str, conn: elmo::Connection, seq: T) -> i64 {
        self.cursor_num = self.cursor_num + 1;
        self.cursors.insert(self.cursor_num, (String::from(ns), conn, box seq));
        self.cursor_num
    }

    fn remove_cursors_for_collection(&mut self, ns: &str) {
        let remove = self.cursors.iter().filter_map(|(&num, &(ref s, _, _))| if s.as_str() == ns { Some(num) } else { None }).collect::<Vec<_>>();
        for cursor_num in remove {
            self.cursors.remove(&cursor_num);
        }
    }

    // grab is just a take() which doesn't take ownership of the iterator
    // TODO investigate by_ref()
    fn grab<T: Iterator<Item=Result<elmo::Row>>>(seq: &mut T, n: usize) -> Result<Vec<elmo::Row>> {
        let mut r = Vec::new();
        for _ in 0 .. n {
            match seq.next() {
                None => {
                    break;
                },
                Some(v) => {
                    r.push(try!(v));
                },
            }
        }
        Ok(r)
    }

    // this is the older way of returning a cursor.
    fn do_limit<T: Iterator<Item=Result<elmo::Row>>>(ns: &str, seq: &mut T, number_to_return: i32) -> Result<(Vec<elmo::Row>, bool)> {
        if number_to_return < 0 || number_to_return == 1 {
            // hard limit.  do not return a cursor.
            let n = if number_to_return < 0 {
                -number_to_return
            } else {
                number_to_return
            };
            if n < 0 {
                // TODO can rust overflow handling deal with this?
                panic!("overflow");
            }
            let docs = try!(seq.take(n as usize).collect::<Result<Vec<_>>>());
            Ok((docs, false))
        } else if number_to_return == 0 {
            // return whatever the default size is
            // TODO for now, just return them all and close the cursor
            let docs = try!(seq.collect::<Result<Vec<_>>>());
            Ok((docs, false))
        } else {
            // soft limit.  keep cursor open.
            let docs = try!(Self::grab(seq, number_to_return as usize));
            if docs.len() == (number_to_return as usize) {
                Ok((docs, true))
            } else {
                Ok((docs, false))
            }
        }
    }

    // this is a newer way of returning a cursor.  used by the agg framework.
    fn reply_with_cursor<T: Iterator<Item=Result<elmo::Row>> + 'static>(&mut self, ns: &str, conn: elmo::Connection, mut seq: T, cursor_options: Option<&bson::Value>, default_batch_size: usize) -> Result<bson::Document> {
        let number_to_return =
            match cursor_options {
                Some(&bson::Value::BDocument(ref bd)) => {
                    if bd.pairs.iter().any(|&(ref k, _)| k != "batchSize") {
                        return Err(Error::Misc(format!("invalid cursor option: {:?}", bd)));
                    }
                    match bd.pairs.iter().find(|&&(ref k, ref _v)| k == "batchSize") {
                        Some(&(_, bson::Value::BInt32(n))) => {
                            if n < 0 {
                                return Err(Error::Misc(String::from("batchSize < 0")));
                            }
                            Some(n as usize)
                        },
                        Some(&(_, bson::Value::BDouble(n))) => {
                            if n < 0.0 {
                                return Err(Error::Misc(String::from("batchSize < 0")));
                            }
                            Some(n as usize)
                        },
                        Some(&(_, bson::Value::BInt64(n))) => {
                            if n < 0 {
                                return Err(Error::Misc(String::from("batchSize < 0")));
                            }
                            Some(n as usize)
                        },
                        Some(_) => {
                            return Err(Error::Misc(String::from("batchSize not numeric")));
                        },
                        None => {
                            Some(default_batch_size)
                        },
                    }
                },
                Some(v) => {
                    return Err(Error::Misc(format!("invalid cursor option: {:?}", v)));
                },
                None => {
                    // TODO in the case where the cursor is not requested, how
                    // many should we return?  For now we return all of them,
                    // which for now we flag by setting number_to_return to None,
                    // which is handled as a special case below.
                    None
                },
        };

        let (docs, cursor_id) =
            match number_to_return {
                None => {
                    let docs = try!(seq.collect::<Result<Vec<_>>>());
                    (docs, 0)
                },
                Some(0) => {
                    // if 0, return nothing but keep the cursor open.
                    // but we need to eval the first item in the seq,
                    // to make sure that an error gets found now.
                    // but we can't consume that first item and let it
                    // get lost.  so we grab a batch but then put it back.

                    // TODO peek, or something
                    let cursor_id = self.store_cursor(ns, conn, seq);
                    (Vec::new(), cursor_id)
                },
                Some(n) => {
                    let docs = try!(Self::grab(&mut seq, n));
                    if docs.len() == n {
                        // if we grabbed the same number we asked for, we assume the
                        // sequence has more, so we store the cursor and return it.
                        let cursor_id = self.store_cursor(ns, conn, seq);
                        (docs, cursor_id)
                    } else {
                        // but if we got less than we asked for, we assume we have
                        // consumed the whole sequence.
                        (docs, 0)
                    }
                },
            };

        let mut doc = bson::Document::new();
        match cursor_options {
            Some(_) => {
                let mut cursor = bson::Document::new();
                cursor.set_i64("id", cursor_id);
                cursor.set_str("ns", ns);
                cursor.set_array("firstBatch", bson::Array { items: vec_rows_to_values(docs)});
                doc.set_document("cursor", cursor);
            },
            None => {
                doc.set_array("result", bson::Array { items: vec_rows_to_values(docs)});
            },
        }
        doc.set_i32("ok", 1);
        Ok(doc)
    }

    fn reply_create_collection(&self, req: &MsgQuery, db: &str) -> Result<Reply> {
        let q = &req.query;
        let coll = try!(req.query.must_get_str("create"));
        let mut options = bson::Document::new();
        // TODO maybe just pass everything through instead of looking for specific options
        match q.get("autoIndexId") {
            Some(&bson::Value::BBoolean(b)) => options.set_bool("autoIndexId", b),
            // TODO error on bad values?
            _ => (),
        }
        match q.get("temp") {
            Some(&bson::Value::BBoolean(b)) => options.set_bool("temp", b),
            // TODO error on bad values?
            _ => (),
        }
        match q.get("capped") {
            Some(&bson::Value::BBoolean(b)) => options.set_bool("capped", b),
            // TODO error on bad values?
            _ => (),
        }
        match q.get("size") {
            Some(&bson::Value::BInt32(n)) => options.set_i64("size", n as i64),
            Some(&bson::Value::BInt64(n)) => options.set_i64("size", n as i64),
            Some(&bson::Value::BDouble(n)) => options.set_i64("size", n as i64),
            // TODO error on bad values?
            _ => (),
        }
        match q.get("max") {
            Some(&bson::Value::BInt32(n)) => options.set_i64("max", n as i64),
            Some(&bson::Value::BInt64(n)) => options.set_i64("max", n as i64),
            Some(&bson::Value::BDouble(n)) => options.set_i64("max", n as i64),
            // TODO error on bad values?
            _ => (),
        }
        // TODO more options here ?
        let result = try!(self.conn.create_collection(db, coll, options));
        let mut doc = bson::Document::new();
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_create_indexes(&mut self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("createIndexes"));
        let indexes = try!(req.query.must_remove_array("indexes"));
        let mut a = vec![];
        for d in indexes.items {
            let mut d = try!(d.into_document());
            let name = try!(d.must_remove_string("name"));
            let spec = try!(d.must_remove_document("key"));
            // TODO look for specific options here like the fs version?
            // anything left in d should be options
            let ndx = 
                elmo::IndexInfo {
                    db: String::from(db),
                    coll: coll.clone(),
                    name: name,
                    spec: spec,
                    options: d,
                };
            a.push(ndx);
        }

        let result = try!(self.conn.create_indexes(a));
        // TODO createdCollectionAutomatically
        // TODO numIndexesBefore
        // TODO numIndexesAfter
        let mut doc = bson::Document::new();
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_delete_indexes(&mut self, req: &MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_get_str("deleteIndexes"));
        {
            // TODO is it safe/correct/necessary to remove the cursors BEFORE?
            let full_coll = format!("{}.{}", db, coll);
            self.remove_cursors_for_collection(&full_coll);
        }
        let index = try!(req.query.must_get("index"));
        let (count_indexes_before, num_indexes_deleted) = try!(self.conn.delete_indexes(db, coll, index));
        let mut doc = bson::Document::new();
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_drop_collection(&mut self, req: &MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_get_str("drop"));
        {
            // TODO is it safe/correct/necessary to remove the cursors BEFORE?
            let full_coll = format!("{}.{}", db, coll);
            self.remove_cursors_for_collection(&full_coll);
        }
        let deleted = try!(self.conn.drop_collection(db, coll));
        let mut doc = bson::Document::new();
        if deleted {
            doc.set_i32("ok", 1);
        } else {
            // mongo shell apparently cares about this exact error message string
            doc.set_str("errmsg", "ns not found");
            doc.set_i32("ok", 0);
        }
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_drop_database(&mut self, req: &MsgQuery, db: &str) -> Result<Reply> {
        // TODO remove cursors?
        let deleted = try!(self.conn.drop_database(db));
        let mut doc = bson::Document::new();
        // apparently this is supposed to return ok=1 whether the db existed or not
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_list_collections(&mut self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let filter = 
            match req.query.remove("filter") {
                Some(v) => Some(try!(v.into_document())),
                None => None,
            };
        let conn = try!(self.factory.open());
        let seq = try!(conn.list_collections(db, filter));

        let default_batch_size = 100;
        let cursor_options = 
            match req.query.remove("cursor") {
                Some(v) => Some(v),
                None => Some(bson::Document::new().into_value()),
            };
        let ns = format!("{}.$cmd.listCollections", db);
        let doc = try!(self.reply_with_cursor(&ns, conn, seq, cursor_options.as_ref(), default_batch_size));
        // note that this uses the newer way of returning a cursor ID, so we pass 0 below
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_list_indexes(&mut self, mut req: MsgQuery, db: &str) -> Result<Reply> {
        let coll = try!(req.query.must_remove_string("listIndexes"));
        if coll.as_str() == "" {
            return Err(Error::Misc(String::from("empty string for argument of listIndexes")));
        }
        let conn = try!(self.factory.open());
        if try!(conn.list_all_collections()).into_iter().filter(|c| c.db == db && c.coll == coll).next().is_none() {
            return Err(Error::Misc(String::from("collection does not exist")));
        }
        let results = try!(conn.list_indexes());
        let seq = {
            // we need db to get captured by this closure which outlives
            // this function, so we create String from it and use a move
            // closure.

            let db = String::from(db);
            let results = results.into_iter().filter_map(
                move |ndx| {
                    if ndx.db.as_str() == db && ndx.coll.as_str() == coll {
                        let mut doc = bson::Document::new();
                        doc.set_string("ns", ndx.full_collection_name());
                        let unique = {
                            match ndx.options.get("unique") {
                                Some(&bson::Value::BBoolean(true)) => true,
                                _ => false,
                            }
                        };
                        // TODO it seems the automatic index on _id is NOT supposed to be marked unique
                        if unique && ndx.name != "_id_" {
                            doc.set_bool("unique", unique);
                        }
                        doc.set_string("name", ndx.name);
                        doc.set_document("key", ndx.spec);
                        let r = elmo::Row {
                            doc: bson::Value::BDocument(doc),
                            pos: None,
                            score: None,
                        };
                        Some(Ok(r))
                    } else {
                        None
                    }
                }
                );
            results
        };

        let default_batch_size = 100;
        let cursor_options = 
            match req.query.remove("cursor") {
                Some(v) => Some(v),
                None => Some(bson::Document::new().into_value()),
            };
        let ns = format!("{}.$cmd.listIndexes", db);
        let doc = try!(self.reply_with_cursor(&ns, conn, seq, cursor_options.as_ref(), default_batch_size));
        // note that this uses the newer way of returning a cursor ID, so we pass 0 below
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    // TODO isn't this the same as bson::split_whatever?
    fn splitname(s: &str) -> Result<(&str, &str)> {
        match s.find('.') {
            None => Err(Error::Misc(String::from("bad namespace"))),
            Some(dot) => Ok((&s[0 .. dot], &s[dot+1 ..]))
        }
    }

    fn try_get_optional_prefix<'a>(v: &'a bson::Document, k: &str) -> Option<&'a bson::Value> {
        assert_eq!(&k[0 .. 1], "$");
        match v.get(k) {
            Some(r) => Some(r),
            None => {
                match v.get(&k[1 ..]) {
                    Some(r) => Some(r),
                    None => None,
                }
            },
        }
    }

    fn try_remove_optional_prefix(v: &mut bson::Document, k: &str) -> Option<bson::Value> {
        assert_eq!(&k[0 .. 1], "$");
        match v.remove(k) {
            Some(r) => Some(r),
            None => {
                match v.remove(&k[1 ..]) {
                    Some(r) => Some(r),
                    None => None,
                }
            },
        }
    }

    fn reply_validate(&mut self, req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;
        let coll = try!(query.must_remove_string("validate"));
        // TODO what is this supposed to actually do?
        let mut doc = bson::Document::new();
        doc.set_bool("valid", true);
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_aggregate(&mut self, req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;
        let coll = try!(query.must_remove_string("aggregate"));
        let pipeline = try!(query.must_remove_array("pipeline"));
        let cursor_options = query.get("cursor");
        match cursor_options {
            Some(&bson::Value::BDocument(_)) => (),
            Some(_) => return Err(Error::Misc(format!("aggregate.cursor must be a document: {:?}", cursor_options))),
            None => (),
        }
        let conn = try!(self.factory.open());
        let (out, seq) = try!(conn.aggregate(db, &coll, pipeline));
        match out {
            Some(new_coll_name) => {
                let full_coll = format!("{}.{}", db, new_coll_name);
                if new_coll_name.starts_with("system.") {
                    return Err(Error::MongoCode(17385, format!("no $out into system coll: {}", new_coll_name)))
                }
                let conn2 = try!(self.factory.open());
                let colls = try!(conn2.list_all_collections());
                let colls = colls.into_iter().filter(|ndx| ndx.db == db && ndx.coll == new_coll_name && ndx.options.get("capped").is_some()).collect::<Vec<_>>();
                if colls.len() > 0 {
                    return Err(Error::MongoCode(17152, format!("no $out into capped coll: {}", new_coll_name)))
                }
                self.remove_cursors_for_collection(&full_coll);
                try!(conn2.clear_collection(db, &new_coll_name));
                let results = try!(conn2.insert_seq(db, &new_coll_name, seq));
                let mut errors = Vec::new();
                for i in 0 .. results.len() {
                    if results[i].is_err() {
                        let msg = format!("{:?}", results[i]);
                        let err = bson::Value::BDocument(bson::Document {pairs: vec![(String::from("index"), bson::Value::BInt32(i as i32)), (String::from("errmsg"), bson::Value::BString(msg))]});
                        errors.push(err);
                    }
                }
                let default_batch_size = 100;
                // TODO conn wasted here.  not even needed by the stored fake cursor.
                let doc = try!(self.reply_with_cursor(&full_coll, conn, std::iter::empty(), cursor_options, default_batch_size));
                // note that this uses the newer way of returning a cursor ID, so we pass 0 below
                Ok(create_reply(req.req_id, vec![doc], 0))
            },
            None => {
                let default_batch_size = 100;
                let ns = format!("{}.{}", db, coll);
                let doc = try!(self.reply_with_cursor(&ns, conn, seq, cursor_options, default_batch_size));
                // note that this uses the newer way of returning a cursor ID, so we pass 0 below
                Ok(create_reply(req.req_id, vec![doc], 0))
            },
        }
    }

    fn reply_count(&mut self, req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;
        let coll = try!(query.must_remove_string("count"));
        let hint = query.remove("hint");
        let q = 
            match query.remove("query") {
                Some(bson::Value::BDocument(bd)) => {
                    bd
                },
                Some(q) => {
                    return Err(Error::Misc(format!("invalid query: {:?}", q)));
                },
                None => {
                    bson::Document::new()
                },
            };
        let seq = try!(self.conn.find(
                db, 
                &coll, 
                q,
                None,
                None,
                None,
                None,
                hint,
                None
                ));
        let mut count = seq.count() as i32;
        match query.remove("skip") {
            None => (),
            Some(n) => {
                let n = try!(n.numeric_to_i32());
                if n < 0 {
                    return Err(Error::Misc(format!("negative skip: {}", n)));
                }
                if count >= n {
                    count = count - n;
                } else {
                    count = 0;
                }
            },
        }
        match query.remove("limit") {
            None => (),
            Some(n) => {
                let mut n = try!(n.numeric_to_i32());
                if n < 0 {
                    n = -n;
                }
                if n > 0 && count > n {
                    count = n;
                }
            },
        }
        let mut doc = bson::Document::new();
        doc.set_i32("n", count as i32);
        doc.set_i32("ok", 1);
        Ok(create_reply(req.req_id, vec![doc], 0))
    }

    fn reply_query(&mut self, req: MsgQuery, db: &str) -> Result<Reply> {
        let MsgQuery {
            req_id,
            flags,
            full_collection_name,
            number_to_skip,
            number_to_return,
            mut query,
            return_fields_selector,
        } = req;

        let (db, coll) = try!(Self::splitname(&full_collection_name));

        // This *might* just have the query in it.  OR it might have the 
        // query in a key called query, which might also be called $query,
        // along with other stuff (like orderby) as well.
        // This other stuff is called query modifiers.  
        // Sigh.

        let conn = try!(self.factory.open());
        let seq = 
            match Self::try_remove_optional_prefix(&mut query, "$query") {
                Some(q) => {
                    // TODO what if somebody queries on a field named query?  ambiguous.

                    let orderby = Self::try_remove_optional_prefix(&mut query, "$orderby");
                    let min = Self::try_remove_optional_prefix(&mut query, "$min");
                    let max = Self::try_remove_optional_prefix(&mut query, "$max");
                    let hint = Self::try_remove_optional_prefix(&mut query, "$hint");
                    let explain = Self::try_remove_optional_prefix(&mut query, "$explain");
                    let q = try!(q.into_document());
                    let seq = try!(conn.find(
                            db, 
                            coll, 
                            q,
                            orderby,
                            return_fields_selector,
                            min,
                            max,
                            hint,
                            explain
                            ));
                    seq
                },
                None => {
                    let seq = try!(conn.find(
                            db, 
                            coll, 
                            query,
                            None,
                            return_fields_selector,
                            None,
                            None,
                            None,
                            None
                            ));
                    seq
                },
            };

        if number_to_skip < 0 {
            return Err(Error::Misc(format!("negative skip: {}", number_to_skip)));
        }

        let seq = seq.skip(number_to_skip as usize);

        let mut seq = seq.map(
            |r| r.map_err(elmo::wrap_err)
        );

        //let docs = try!(Self::grab(&mut seq, number_to_return as usize));
        //Ok(create_reply(req_id, docs, 0))

        let (docs, more) = try!(Self::do_limit(&full_collection_name, &mut seq, number_to_return));
        let cursor_id = if more {
            self.store_cursor(&full_collection_name, conn, seq)
            //0
        } else {
            // TODO conn wasted here
            0
        };
        let docs = vec_rows_to_values(docs);
        let docs = try!(vec_values_to_docs(docs));
        Ok(create_reply(req_id, docs, cursor_id))
    }

    fn reply_cmd(&mut self, req: MsgQuery, db: &str) -> Result<Reply> {
        use std::ascii::AsciiExt;
        if req.query.pairs.is_empty() {
            Err(Error::Misc(String::from("empty query")))
        } else {
            // this code assumes that the first key is always the command
            let cmd = req.query.pairs[0].0.clone().to_ascii_lowercase();
            let res =
                // TODO isMaster needs to be in here?
                match cmd.as_str() {
                    "profile" => self.reply_profile(req, db),
                    "collstats" => self.reply_collstats(req, db),
                    "explain" => self.reply_explain(req, db),
                    "aggregate" => self.reply_aggregate(req, db),
                    "insert" => self.reply_insert(req, db),
                    "delete" => self.reply_delete(req, db),
                    "distinct" => self.reply_distinct(req, db),
                    "update" => self.reply_update(req, db),
                    "findandmodify" => self.reply_find_and_modify(req, db),
                    "count" => self.reply_count(req, db),
                    "validate" => self.reply_validate(req, db),
                    "createindexes" => self.reply_create_indexes(req, db),
                    "deleteindexes" => self.reply_delete_indexes(&req, db),
                    "drop" => self.reply_drop_collection(&req, db),
                    "dropdatabase" => self.reply_drop_database(&req, db),
                    "listcollections" => self.reply_list_collections(req, db),
                    "listindexes" => self.reply_list_indexes(req, db),
                    "create" => self.reply_create_collection(&req, db),
                    //"features" => reply_features &req db
                    _ => Err(Error::Misc(format!("unknown cmd: {}", cmd)))
                };
            res
        }
    }

    fn reply_2004(&mut self, req: MsgQuery) -> Result<Reply> {
        // reallocating the strings here so we can pass ownership of req down the line.
        // TODO we could deconstruct req now?
        let parts = req.full_collection_name.split('.').map(|s| String::from(s)).collect::<Vec<_>>();
        // TODO check for bad collection name here
        let req_id = req.req_id;
        let r = 
            if parts.len() < 2 {
                Err(Error::Misc(format!("bad collection name: {}", req.full_collection_name)))
            } else {
                let db = &parts[0];
                if db == "admin" {
                    if parts[1] == "$cmd" {
                        //reply_AdminCmd req
                        // TODO probably want to pass ownership of req down here
                        self.reply_admin_cmd(&req, db)
                    } else {
                        Err(Error::Misc(format!("TODO: {:?}", req)))
                    }
                } else {
                    if parts[1] == "$cmd" {
                        if parts.len() == 4 && parts[2]=="sys" && parts[3]=="inprog" {
                            self.reply_cmd_sys_inprog(&req, db)
                        } else {
                            self.reply_cmd(req, db)
                        }
                    } else if parts.len()==3 && parts[1]=="system" && parts[2]=="indexes" {
                        //reply_system_indexes req db
                        Err(Error::Misc(format!("TODO: {:?}", req)))
                    } else if parts.len()==3 && parts[1]=="system" && parts[2]=="namespaces" {
                        //reply_system_namespaces req db
                        Err(Error::Misc(format!("TODO: {:?}", req)))
                    } else {
                        match self.reply_query(req, db) {
                            Ok(r) => Ok(r),
                            Err(e) => Ok(reply_err(req_id, e)),
                        }
                    }
                }
            };
        //println!("reply: {:?}", r);
        r
    }

    fn reply_2005(&mut self, req: MsgGetMore) -> Reply {
        match self.cursors.remove(&req.cursor_id) {
            Some((ns, conn, mut seq)) => {
                match Self::do_limit(&ns, &mut seq, req.number_to_return) {
                    Ok((docs, more)) => {
                        if more {
                            // put the cursor back for next time
                            self.cursors.insert(req.cursor_id, (ns, conn, box seq));
                        } else {
                            // TODO conn wasted here
                        }
                        let docs = vec_rows_to_values(docs);
                        match vec_values_to_docs(docs) {
                            Ok(docs) => {
                                create_reply(req.req_id, docs, if more { req.cursor_id } else { 0 })
                            },
                            Err(e) => {
                                reply_err(req.req_id, e)
                            },
                        }
                    },
                    Err(e) => {
                        reply_err(req.req_id, e)
                    },
                }
            },
            None => {
                let mut r = create_reply(req.req_id, vec![], 0);
                r.flags = 1;
                r
            },
        }
    }

    fn handle_one_message(&mut self, stream: &mut std::net::TcpStream) -> Result<bool> {
        fn send_reply(stream: &mut std::net::TcpStream, resp: Reply) -> Result<bool> {
            let ba = resp.encode();
            let wrote = try!(misc::io::write_fully(stream, &ba));
            if wrote != ba.len() {
                return Err(Error::Misc(String::from("network write failed")));
            } else {
                Ok(true)
            }
        }

        let ba = try!(read_message_bytes(stream));
        match ba {
            None => {
                //println!("no request");
                Ok(false)
            },
            Some(ba) => {
                let msg = try!(parse_request(&ba));
                //println!("request: {:?}", msg);
                match msg {
                    Request::KillCursors(req) => {
                        for cursor_id in req.cursor_ids {
                            self.cursors.remove(&cursor_id);
                        }
                        // there is no reply to this
                        Ok(true)
                    },
                    Request::Query(req) => {
                        // TODO so if we clear all the cursors here, count2.js passes.
                        //self.cursors.clear();
                        let req_id = req.req_id;
                        let resp = 
                            match self.reply_2004(req) {
                                Ok(r) => r,
                                Err(e) => reply_errmsg(req_id, e),
                            };
                        send_reply(stream, resp)
                    },
                    Request::GetMore(req) => {
                        let resp = self.reply_2005(req);
                        //println!("2005 reply: {:?}", resp);
                        send_reply(stream, resp)
                    },
                }
            }
        }
    }

    fn handle_client(&mut self, mut stream: std::net::TcpStream) -> Result<()> {
        loop {
            match self.handle_one_message(&mut stream) {
                Ok(false) => {
                    return Ok(());
                },
                Ok(true) => {
                    // keep going
                },
                Err(e) => {
                    return Err(e);
                },
            }
        }
    }

}

// TODO args:  ipaddr, port
pub fn serve(factory: Box<elmo::ConnectionFactory>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:27017").unwrap();

    // accept connections and process them, spawning a new thread for each one
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let factory = factory.clone_for_new_thread();
                // TODO thread::spawn panics when the OS cannot create
                // a thread.  use thread::Builder::spawn() instead.
                std::thread::spawn(move || {
                    // connection succeeded
                    let mut s = Server::new(factory);
                    s.handle_client(stream).expect("TODO");
                });
            }
            Err(e) => { /* connection failed */ }
        }
    }

    // close the socket server
    drop(listener);
}

pub fn main() {
    match elmo_lsm::MyFactory::new(String::from("elmodata.lsm")) {
        Ok(factory) => {
            serve(box factory);
        },
        Err(e) => {
            println!("Error: {}", e);
            panic!();
        },
    }
}

