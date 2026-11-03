//! A `bun_sema::util::ShardedMap` keeps every entry while its shards' chunks grow from 16 entries
//! (`vendor/patches/bun-sema-small-shard-chunks.patch`). Bun's own tests cover the chunk layout,
//! not the map.

use bun_sema::util::ShardedMap;

#[test]
fn a_sharded_map_finds_every_entry_across_its_chunks() {
    // About 300 entries in each of the 256 shards: past chunks of 16, 32, 64 and 128 entries.
    const COUNT: u64 = 80_000;
    let map: ShardedMap<u64, u64> = ShardedMap::default();
    std::thread::scope(|scope| {
        for thread in 0..4 {
            let map = &map;
            scope.spawn(move || {
                for key in (thread..COUNT).step_by(4) {
                    assert_eq!(*map.insert_ref(key, key * 3), key * 3);
                }
            });
        }
    });
    assert_eq!(map.entries(), COUNT as usize);
    for key in 0..COUNT {
        assert_eq!(map.get_ref(&key), Some(&(key * 3)));
    }
    assert_eq!(*map.insert_ref(7, 0), 21);
    assert_eq!(map.get_ref(&COUNT), None);
}
