//! DB mock for testing

use std::collections::{btree_map, BTreeMap, HashMap};
use std::ops::Bound::{Excluded, Included};

use super::{BlockState, DBIter, Error, Result, DB};
use crate::ledger::storage::types::{self, KVBytes, PrefixIterator};
use crate::types::address::Address;
use crate::types::storage::{
    BlockHeight, Key, KeySeg, KEY_SEGMENT_SEPARATOR, RESERVED_VP_KEY,
};
use crate::types::time::DateTimeUtc;

/// An in-memory DB for testing.
#[derive(Debug)]
pub struct MockDB(BTreeMap<String, Vec<u8>>);

impl Default for MockDB {
    fn default() -> MockDB {
        MockDB(BTreeMap::new())
    }
}

impl DB for MockDB {
    fn flush(&self) -> Result<()> {
        Ok(())
    }

    fn write_block(&mut self, state: BlockState) -> Result<()> {
        // Epoch start height and time
        self.0.insert(
            "epoch_start_height".into(),
            types::encode(&state.epoch_start_height),
        );
        self.0.insert(
            "epoch_start_time".into(),
            types::encode(&state.epoch_start_time),
        );

        let prefix_key = Key::from(state.height.to_db_key());
        // Merkle tree
        {
            let prefix_key = prefix_key
                .push(&"tree".to_owned())
                .map_err(Error::KeyError)?;
            // Merkle root hash
            {
                let key = prefix_key
                    .push(&"root".to_owned())
                    .map_err(Error::KeyError)?;
                let value = &state.root;
                self.0.insert(key.to_string(), types::encode(value));
            }
            // Tree's store
            {
                let key = prefix_key
                    .push(&"store".to_owned())
                    .map_err(Error::KeyError)?;
                let value = &state.store;
                self.0.insert(key.to_string(), types::encode(value));
            }
        }
        // Block hash
        {
            let key = prefix_key
                .push(&"hash".to_owned())
                .map_err(Error::KeyError)?;
            let value = &state.hash;
            self.0.insert(key.to_string(), types::encode(value));
        }
        // Block epoch
        {
            let key = prefix_key
                .push(&"epoch".to_owned())
                .map_err(Error::KeyError)?;
            let value = &state.epoch;
            self.0.insert(key.to_string(), types::encode(value));
        }
        // SubSpace
        {
            let subspace_prefix = prefix_key
                .push(&"subspace".to_owned())
                .map_err(Error::KeyError)?;
            state.subspaces.iter().for_each(|(key, value)| {
                let key = subspace_prefix.join(key);
                self.0.insert(key.to_string(), value.clone());
            });
        }
        // Address gen
        {
            let key = prefix_key
                .push(&"address_gen".to_owned())
                .map_err(Error::KeyError)?;
            let value = &state.address_gen;
            self.0.insert(key.to_string(), types::encode(value));
        }
        self.0
            .insert("height".to_owned(), types::encode(&state.height));
        Ok(())
    }

    fn read(&self, height: BlockHeight, key: &Key) -> Result<Option<Vec<u8>>> {
        let key = Key::from(height.to_db_key())
            .push(&"subspace".to_owned())
            .map_err(Error::KeyError)?
            .join(key);
        match self.0.get(&key.to_string()) {
            Some(v) => Ok(Some(v.clone())),
            None => Ok(None),
        }
    }

    fn read_last_block(&mut self) -> Result<Option<BlockState>> {
        // Block height
        let height: BlockHeight;
        match self.0.get("height") {
            Some(bytes) => {
                height = types::decode(bytes).map_err(Error::CodingError)?;
            }
            None => return Ok(None),
        }

        // Epoch start height and time
        let epoch_start_height: BlockHeight = match self
            .0
            .get("epoch_start_height")
        {
            Some(bytes) => types::decode(bytes).map_err(Error::CodingError)?,
            None => return Ok(None),
        };
        let epoch_start_time: DateTimeUtc = match self.0.get("epoch_start_time")
        {
            Some(bytes) => types::decode(bytes).map_err(Error::CodingError)?,
            None => return Ok(None),
        };

        // Load data at the height
        let prefix = format!("{}/", height.raw());
        let upper_prefix = format!("{}/", height.next_height().raw());
        let mut root = None;
        let mut store = None;
        let mut hash = None;
        let mut epoch = None;
        let mut address_gen = None;
        let mut subspaces: HashMap<Key, Vec<u8>> = HashMap::new();
        for (path, bytes) in
            self.0.range((Included(prefix), Excluded(upper_prefix)))
        {
            let mut segments: Vec<&str> =
                path.split(KEY_SEGMENT_SEPARATOR).collect();
            match segments.get(1) {
                Some(prefix) => {
                    match *prefix {
                        "tree" => match segments.get(2) {
                            Some(smt) => match *smt {
                                "root" => {
                                    root = Some(
                                        types::decode(bytes)
                                            .map_err(Error::CodingError)?,
                                    )
                                }
                                "store" => {
                                    store = Some(
                                        types::decode(bytes)
                                            .map_err(Error::CodingError)?,
                                    )
                                }
                                _ => unknown_key_error(path)?,
                            },
                            None => unknown_key_error(path)?,
                        },
                        "hash" => {
                            hash = Some(
                                types::decode(bytes)
                                    .map_err(Error::CodingError)?,
                            )
                        }
                        "epoch" => {
                            epoch = Some(
                                types::decode(bytes)
                                    .map_err(Error::CodingError)?,
                            )
                        }
                        "subspace" => {
                            // We need special handling of validity predicate
                            // keys, which are reserved and so calling
                            // `Key::parse` on them would fail
                            let key = match segments.get(3) {
                                Some(seg) if *seg == RESERVED_VP_KEY => {
                                    // the path of a validity predicate should
                                    // be height/subspace/address/?
                                    let mut addr_str = (*segments
                                        .get(2)
                                        .expect("the address not found"))
                                    .to_owned();
                                    let _ = addr_str.remove(0);
                                    let addr = Address::decode(&addr_str)
                                        .expect("cannot decode the address");
                                    Key::validity_predicate(&addr)
                                }
                                _ => {
                                    Key::parse(segments.split_off(2).join(
                                        &KEY_SEGMENT_SEPARATOR.to_string(),
                                    ))
                                    .map_err(|e| Error::Temporary {
                                        error: format!(
                                            "Cannot parse key segments {}: {}",
                                            path, e
                                        ),
                                    })?
                                }
                            };
                            subspaces.insert(key, bytes.to_vec());
                        }
                        "address_gen" => {
                            address_gen = Some(
                                types::decode(bytes)
                                    .map_err(Error::CodingError)?,
                            );
                        }
                        _ => unknown_key_error(path)?,
                    }
                }
                None => unknown_key_error(path)?,
            }
        }
        match (root, store, hash, epoch, address_gen) {
            (
                Some(root),
                Some(store),
                Some(hash),
                Some(epoch),
                Some(address_gen),
            ) => Ok(Some(BlockState {
                root,
                store,
                hash,
                height,
                epoch,
                epoch_start_height,
                epoch_start_time,
                subspaces,
                address_gen,
            })),
            _ => Err(Error::Temporary {
                error: "Essential data couldn't be read from the DB"
                    .to_string(),
            }),
        }
    }
}

impl<'iter> DBIter<'iter> for MockDB {
    type PrefixIter = MockPrefixIterator<'iter>;

    fn iter_prefix(
        &'iter self,
        height: BlockHeight,
        prefix: &Key,
    ) -> MockPrefixIterator<'iter> {
        let db_prefix = format!("{}/subspace/", height.raw());
        let prefix = format!("{}{}", db_prefix, prefix.to_string());
        let iter = self.0.iter();
        MockPrefixIterator::new(MockIterator { prefix, iter }, db_prefix)
    }
}

/// A prefix iterator base for the [`MockPrefixIterator`].
#[derive(Debug)]
pub struct MockIterator<'a> {
    prefix: String,
    /// The concrete iterator
    pub iter: btree_map::Iter<'a, String, Vec<u8>>,
}

/// A prefix iterator for the [`MockDB`].
pub type MockPrefixIterator<'a> = PrefixIterator<MockIterator<'a>>;

impl<'a> Iterator for MockIterator<'a> {
    type Item = KVBytes;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((key, val)) = self.iter.next() {
            if key.starts_with(&self.prefix) {
                return Some((
                    Box::from(key.as_bytes()),
                    Box::from(val.as_slice()),
                ));
            }
        }
        None
    }
}

impl<'a> Iterator for PrefixIterator<MockIterator<'a>> {
    type Item = (String, Vec<u8>, u64);

    /// Returns the next pair and the gas cost
    fn next(&mut self) -> Option<(String, Vec<u8>, u64)> {
        match self.iter.next() {
            Some((key, val)) => {
                let key = String::from_utf8(key.to_vec())
                    .expect("Cannot convert from bytes to key string");
                match key.strip_prefix(&self.db_prefix) {
                    Some(k) => {
                        let gas = k.len() + val.len();
                        Some((k.to_owned(), val.to_vec(), gas as _))
                    }
                    None => self.next(),
                }
            }
            None => None,
        }
    }
}

fn unknown_key_error(key: &str) -> Result<()> {
    Err(Error::UnknownKey {
        key: key.to_owned(),
    })
}
