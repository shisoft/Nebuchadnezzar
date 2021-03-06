use bifrost::raft::client::RaftClient;
use bifrost::raft::state_machine::master::ExecError;
use bifrost_hasher::hash_str;

use dovahkiin::types::Type;
use lightning::map::{HashMap as LFHashMap, Map, ObjectMap};
use std::collections::HashMap;
use std::mem;
use std::sync::atomic::AtomicU32;

use super::types;
use core::borrow::Borrow;
use std::string::String;
use std::sync::Arc;

use futures::prelude::*;
use futures::FutureExt;
use std::ops::Deref;

pub mod sm;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Schema {
    pub id: u32,
    pub name: String,
    pub key_field: Option<Vec<u64>>,
    pub str_key_field: Option<Vec<String>>,
    pub field_index: HashMap<u64, Vec<usize>>,
    pub id_index: HashMap<u64, Vec<u64>>,
    pub index_fields: HashMap<u64, Vec<IndexType>>,
    pub fields: Field,
    pub static_bound: usize,
    pub is_dynamic: bool,
    pub is_scannable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    Ranged,
    Hashed,
    Vectorized,
    Statistics,
}

impl Schema {
    pub fn new(
        name: &str,
        key_field: Option<Vec<String>>,
        mut fields: Field,
        is_dynamic: bool,
        is_scannable: bool,
    ) -> Schema {
        let mut bound = 0;
        let mut field_index = HashMap::new();
        let mut id_index = HashMap::new();
        let mut index_fields = HashMap::new();
        fields.assign_offsets(
            &mut bound,
            &mut field_index,
            &mut id_index,
            &mut index_fields,
            String::new(),
            vec![],
            vec![],
        );
        trace!("Schema {:?} has bound {}", fields, bound);
        Schema {
            id: 0,
            name: name.to_string(),
            key_field: match key_field {
                None => None,
                Some(ref keys) => Some(keys.iter().map(|f| hash_str(f)).collect()), // field list into field ids
            },
            str_key_field: key_field,
            static_bound: bound,
            fields,
            is_dynamic,
            is_scannable,
            field_index,
            id_index,
            index_fields,
        }
    }
    pub fn new_with_id(
        id: u32,
        name: &str,
        key_field: Option<Vec<String>>,
        fields: Field,
        dynamic: bool,
        scannable: bool,
    ) -> Schema {
        let mut schema = Schema::new(name, key_field, fields, dynamic, scannable);
        schema.id = id;
        schema
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub data_type: Type,
    pub nullable: bool,
    pub is_array: bool,
    pub sub_fields: Option<Vec<Field>>,
    pub name: String,
    pub name_id: u64,
    pub indices: Vec<IndexType>,
    pub offset: Option<usize>,
}

impl Field {
    pub fn new(
        name: &str,
        data_type: Type,
        nullable: bool,
        is_array: bool,
        sub_fields: Option<Vec<Field>>,
        indices: Vec<IndexType>,
    ) -> Field {
        Field {
            name: name.to_string(),
            name_id: types::key_hash(name),
            data_type,
            nullable,
            is_array,
            sub_fields,
            indices,
            offset: None,
        }
    }
    fn assign_offsets(
        &mut self,
        offset: &mut usize,
        field_index: &mut HashMap<u64, Vec<usize>>,
        id_index: &mut HashMap<u64, Vec<u64>>,
        index_fields: &mut HashMap<u64, Vec<IndexType>>,
        name_path: String,
        field_path: Vec<usize>,
        id_path: Vec<u64>,
    ) {
        const POINTER_SIZE: usize = mem::size_of::<u32>();
        self.offset = Some(*offset);
        let is_field_var = self.is_var();
        let name_path_hash = hash_str(&name_path);
        if self.nullable && !is_field_var {
            *offset += 1;
        }
        if self.is_array {
            // u32 as indication of the offset to the actual data
            *offset += POINTER_SIZE;
        } else if let Some(ref mut subs) = self.sub_fields {
            let format_name = if name_path.is_empty() {
                name_path
            } else {
                format!("{}|", name_path)
            };
            subs.iter_mut().enumerate().for_each(|(i, f)| {
                let mut new_path = field_path.clone();
                let mut new_id = id_path.clone();
                new_path.push(i);
                new_id.push(f.name_id);
                let new_name_path = format!("{}{}", format_name, f.name);
                f.assign_offsets(
                    offset,
                    field_index,
                    id_index,
                    index_fields,
                    new_name_path,
                    new_path,
                    new_id,
                );
            });
        } else {
            if !is_field_var {
                *offset += types::size_of_type(self.data_type);
            } else {
                *offset += POINTER_SIZE;
            }
        }
        if !field_path.is_empty() {
            field_index.insert(name_path_hash, field_path);
        }
        if !id_path.is_empty() {
            id_index.insert(name_path_hash, id_path);
            if !self.indices.is_empty() {
                index_fields.insert(name_path_hash, self.indices.clone());
            }
        }
        trace!(
            "Assigned field {} to {:?}, now at {}, var {}, offset moved {}",
            self.name,
            self.offset,
            offset,
            is_field_var,
            *offset - self.offset.unwrap()
        );
    }
    pub fn is_var(&self) -> bool {
        self.is_array || !types::fixed_size(self.data_type)
    }
}

pub struct SchemasMap {
    schema_map: ObjectMap<SchemaRef>,
    name_map: LFHashMap<String, usize>,
    id_counter: AtomicU32,
}

pub struct LocalSchemasCache {
    map: Arc<SchemasMap>,
}

impl LocalSchemasCache {
    pub async fn new(
        group: &str,
        raft_client: &Arc<RaftClient>,
    ) -> Result<LocalSchemasCache, ExecError> {
        info!("Initializing local schema cache");
        let map = Arc::new(SchemasMap::new());
        let m1 = map.clone();
        let m2 = map.clone();
        let sm = sm::client::SMClient::new(sm::generate_sm_id(group), raft_client);
        let sm_data = sm.get_all().await?;
        {
            debug!("Importing {} schemas from cluster", sm_data.len());
            for schema in sm_data {
                trace!("Importing schema {}", schema.name);
                map.new_schema(schema);
            }
        }
        debug!("Subscribing schema events...");
        let _ = sm
            .on_schema_added(move |schema| {
                debug!("Add schema {} from subscription", schema.id);
                m1.new_schema(schema);
                future::ready(()).boxed()
            })
            .await?;
        let _ = sm
            .on_schema_deleted(move |schema| {
                m2.del_schema(&schema).unwrap();
                future::ready(()).boxed()
            })
            .await?;
        let schemas = LocalSchemasCache { map };
        info!("Local schema initialization completed");
        return Ok(schemas);
    }
    pub fn new_local(_group: &str) -> Self {
        let map = Arc::new(SchemasMap::new());
        LocalSchemasCache { map }
    }
    pub fn get(&self, id: &u32) -> Option<SchemaRef> {
        self.map.get(id)
    }
    pub fn new_schema(&self, schema: Schema) {
        // for debug only
        let mut m = &self.map;
        m.new_schema(schema)
    }
    pub fn name_to_id(&self, name: &str) -> Option<u32> {
        let m = &self.map;
        m.name_to_id(name)
    }
}

impl SchemasMap {
    pub fn new() -> SchemasMap {
        SchemasMap {
            schema_map: ObjectMap::with_capacity(32),
            name_map: LFHashMap::with_capacity(32),
            id_counter: AtomicU32::new(0),
        }
    }
    pub fn new_schema(&self, schema: Schema) {
        let name = &schema.name;
        let id = schema.id;
        self.name_map.insert(name, id as usize);
        self.schema_map.insert(&(id as usize), Arc::new(schema));
    }
    pub fn del_schema(&self, name: &str) -> Result<(), ()> {
        if let Some(id) = self.name_map.remove(&(name.to_owned())) {
            self.schema_map.remove(&id);
        }
        Ok(())
    }
    pub fn get_by_name(&self, name: &str) -> Option<SchemaRef> {
        if let Some(id) = self.name_to_id(name) {
            return self.get(&id);
        }
        return None;
    }
    pub fn get(&self, id: &u32) -> Option<SchemaRef> {
        self.schema_map.get(&(*id as usize))
    }
    pub fn name_to_id(&self, name: &str) -> Option<u32> {
        self.name_map.get(&name.to_string()).map(|id| id as u32)
    }
    fn next_id(&mut self) -> u32 {
        let mut id = self
            .id_counter
            .fetch_and(1, std::sync::atomic::Ordering::AcqRel);
        while self.schema_map.contains_key(&(id as usize)) {
            id = self
                .id_counter
                .fetch_and(1, std::sync::atomic::Ordering::AcqRel)
        }
        id
    }
    fn get_all(&self) -> Vec<Schema> {
        self.schema_map
            .entries()
            .iter()
            .map(|(_, s_ref)| (**s_ref).clone())
            .collect()
    }
    fn load_from_list(&mut self, data: Vec<Schema>) {
        for schema in data {
            let id = schema.id as usize;
            self.name_map.insert(&schema.name, id);
            self.schema_map.insert(&id, Arc::new(schema));
        }
    }
}

pub struct ReadingRef<O, T: ?Sized> {
    _owner: O,
    reference: *const T,
}

pub type SchemaRef = Arc<Schema>;

impl<O, T: ?Sized> Deref for ReadingRef<O, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.reference }
    }
}
