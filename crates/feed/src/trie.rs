//! A cached Patricia tree for repeated checkpoint verification. Hashing rules
//! are shared with `mpt`; full recomputation remains the independent oracle.
use crate::{mpt, FeedError, Felt};
use std::collections::BTreeMap;

type Key = [u8; 32];

#[derive(Clone, Debug)]
enum Node {
    Leaf {
        key: Key,
        value: Felt,
    },
    Branch {
        key: Key,
        depth: usize,
        hash: Felt,
        left: Box<Node>,
        right: Box<Node>,
    },
}

fn bit(key: &Key, depth: usize) -> bool {
    let n = 250 - depth;
    key[31 - n / 8] & (1 << (n % 8)) != 0
}

fn path(key: &Key, from: usize, to: usize) -> Felt {
    let mut out = [0u8; 32];
    for d in from..to {
        if bit(key, d) {
            let n = to - 1 - d;
            out[31 - n / 8] |= 1 << (n % 8);
        }
    }
    Felt::from_bytes_be(&out)
}

impl Node {
    fn key(&self) -> &Key {
        match self {
            Self::Leaf { key, .. } | Self::Branch { key, .. } => key,
        }
    }
    fn hash_at(&self, start: usize) -> Felt {
        let (depth, hash) = match self {
            Self::Leaf { value, .. } => (251, *value),
            Self::Branch { depth, hash, .. } => (*depth, *hash),
        };
        if start == depth {
            hash
        } else {
            mpt::edge_hash(
                &hash,
                &path(self.key(), start, depth),
                (depth - start) as u64,
            )
        }
    }
    fn branch(depth: usize, left: Box<Self>, right: Box<Self>) -> Box<Self> {
        Box::new(Self::Branch {
            key: *left.key(),
            depth,
            hash: mpt::binary_hash(&left.hash_at(depth + 1), &right.hash_at(depth + 1)),
            left,
            right,
        })
    }
    fn build(entries: &[(Key, Felt)], start: usize) -> Box<Self> {
        if entries.len() == 1 {
            return Box::new(Self::Leaf {
                key: entries[0].0,
                value: entries[0].1,
            });
        }
        let depth = (start..251)
            .find(|d| bit(&entries[0].0, *d) != bit(&entries[entries.len() - 1].0, *d))
            .unwrap();
        let split = entries.partition_point(|(key, _)| !bit(key, depth));
        Self::branch(
            depth,
            Self::build(&entries[..split], depth + 1),
            Self::build(&entries[split..], depth + 1),
        )
    }
    fn update(self: Box<Self>, key: Key, value: Felt, start: usize) -> Option<Box<Self>> {
        let depth = match &*self {
            Self::Leaf { .. } => 251,
            Self::Branch { depth, .. } => *depth,
        };
        if let Some(split) = (start..depth).find(|d| bit(self.key(), *d) != bit(&key, *d)) {
            if value == Felt::ZERO {
                return Some(self);
            }
            let leaf = Box::new(Self::Leaf { key, value });
            return Some(if bit(&key, split) {
                Self::branch(split, self, leaf)
            } else {
                Self::branch(split, leaf, self)
            });
        }
        match *self {
            Self::Leaf { .. } => (value != Felt::ZERO).then(|| Box::new(Self::Leaf { key, value })),
            Self::Branch {
                depth, left, right, ..
            } => {
                if bit(&key, depth) {
                    match right.update(key, value, depth + 1) {
                        Some(right) => Some(Self::branch(depth, left, right)),
                        None => Some(left),
                    }
                } else {
                    match left.update(key, value, depth + 1) {
                        Some(left) => Some(Self::branch(depth, left, right)),
                        None => Some(right),
                    }
                }
            }
        }
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Leaf { key, value } => {
                out.push(0);
                out.extend(key);
                out.extend(value.to_bytes_be());
            }
            Self::Branch {
                depth,
                hash,
                left,
                right,
                ..
            } => {
                out.push(1);
                out.extend((*depth as u16).to_be_bytes());
                out.extend(hash.to_bytes_be());
                left.encode(out);
                right.encode(out);
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CachedTrie {
    root: Option<Box<Node>>,
    values: BTreeMap<Key, Felt>,
}

impl CachedTrie {
    pub fn root(&self) -> Felt {
        self.root.as_ref().map_or(Felt::ZERO, |n| n.hash_at(0))
    }

    /// Compare complete slot sets, but hash only changed paths. Handles zero
    /// writes and deletions as well as append-only inserts and reorg rollback.
    pub fn update(&mut self, entries: &[(Felt, Felt)]) -> Result<Felt, FeedError> {
        let mut next = BTreeMap::new();
        for (key, value) in entries {
            if !mpt::is_trie_key(key) {
                return Err(FeedError::Malformed("invalid trie key".into()));
            }
            if *value != Felt::ZERO && next.insert(key.to_bytes_be(), *value).is_some() {
                return Err(FeedError::Malformed("duplicate trie key".into()));
            }
        }
        if self.root.is_none() {
            if !next.is_empty() {
                self.root = Some(Node::build(
                    &next.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
                    0,
                ));
            }
        } else {
            for (key, value) in next.iter().filter(|(k, v)| self.values.get(*k) != Some(*v)) {
                self.root = match self.root.take() {
                    Some(root) => root.update(*key, *value, 0),
                    None => Some(Box::new(Node::Leaf {
                        key: *key,
                        value: *value,
                    })),
                };
            }
            for key in self.values.keys().filter(|k| !next.contains_key(*k)) {
                if let Some(root) = self.root.take() {
                    self.root = root.update(*key, Felt::ZERO, 0);
                }
            }
        }
        self.values = next;
        Ok(self.root())
    }

    /// Local cache format; hashes are trusted, not authenticated by this codec.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.encode(&mut out);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FeedError> {
        fn node(
            input: &mut &[u8],
            min_depth: usize,
            values: &mut BTreeMap<Key, Felt>,
        ) -> Option<Box<Node>> {
            let tag = take::<1>(input)?[0];
            match tag {
                0 => {
                    let key = take::<32>(input)?;
                    let value = Felt::from_bytes_be(&take::<32>(input)?);
                    if value == Felt::ZERO
                        || !mpt::is_trie_key(&Felt::from_bytes_be(&key))
                        || values.insert(key, value).is_some()
                    {
                        return None;
                    }
                    Some(Box::new(Node::Leaf { key, value }))
                }
                1 => {
                    let depth = u16::from_be_bytes(take::<2>(input)?) as usize;
                    if depth < min_depth || depth >= 251 {
                        return None;
                    }
                    let hash = Felt::from_bytes_be(&take::<32>(input)?);
                    let left = node(input, depth + 1, values)?;
                    let right = node(input, depth + 1, values)?;
                    Some(Box::new(Node::Branch {
                        key: *left.key(),
                        depth,
                        hash,
                        left,
                        right,
                    }))
                }
                _ => None,
            }
        }
        fn take<const N: usize>(input: &mut &[u8]) -> Option<[u8; N]> {
            let result = input.get(..N)?.try_into().ok()?;
            *input = &input[N..];
            Some(result)
        }
        let mut out = Self::default();
        if bytes.is_empty() {
            return Ok(out);
        }
        let mut remaining = bytes;
        out.root = node(&mut remaining, 0, &mut out.values);
        if out.root.is_none() || !remaining.is_empty() {
            return Err(FeedError::Malformed("corrupt cached trie".into()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incremental_and_restored_roots_match_full_recomputation() {
        let mut tree = CachedTrie::default();
        let mut slots = BTreeMap::new();
        let mut rng = 19u64;
        for round in 0..80 {
            for _ in 0..8 {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let key = Felt::from((rng >> 32) % 128);
                if rng % 4 == 0 {
                    slots.remove(&key);
                } else {
                    slots.insert(key, Felt::from(rng));
                }
            }
            let entries: Vec<_> = slots.iter().map(|(k, v)| (*k, *v)).collect();
            assert_eq!(
                tree.update(&entries).unwrap(),
                mpt::storage_root(&entries),
                "round {round}"
            );
            tree = CachedTrie::decode(&tree.encode()).unwrap();
            assert_eq!(tree.root(), mpt::storage_root(&entries));
        }
        assert_eq!(tree.update(&[]).unwrap(), Felt::ZERO);
    }
}
