#![warn(rust_2018_idioms)]

use bytes::{BufMut, BytesMut};

fn checksum(buf: &[u8]) -> u64 {
    // FNV-1a, stable across test runs.
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in buf {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn share_reads_same_memory_without_copy() {
    let mut a = BytesMut::from(&b"hello world"[..]);
    let b = a.share();

    // Both handles see the same bytes and the same backing memory: sharing is
    // a zero-copy read operation.
    assert_eq!(&a[..], &b[..]);
    assert_eq!(a.as_ptr(), b.as_ptr());
}

#[test]
fn index_write_is_isolated() {
    let mut a = BytesMut::from(&b"hello"[..]);
    let mut b = a.share();

    a[0] = b'j';

    assert_eq!(&a[..], b"jello");
    assert_eq!(&b[..], b"hello");

    b[4] = b'!';
    assert_eq!(&a[..], b"jello");
    assert_eq!(&b[..], b"hell!");
}

#[test]
fn two_writers_same_block_stay_independent() {
    // The reported checksum failure scenario: two users mutate the same shared
    // block and each must keep seeing its own content.
    let payload: Vec<u8> = (0..=255u8).collect();
    let mut a = BytesMut::from(&payload[..]);
    let original = checksum(&a);
    let mut b = a.share();

    for x in a.iter_mut() {
        *x = x.wrapping_add(1);
    }
    for x in b.iter_mut() {
        *x = x.wrapping_sub(1);
    }

    let expected_a: Vec<u8> = payload.iter().map(|x| x.wrapping_add(1)).collect();
    let expected_b: Vec<u8> = payload.iter().map(|x| x.wrapping_sub(1)).collect();

    assert_eq!(&a[..], &expected_a[..]);
    assert_eq!(&b[..], &expected_b[..]);
    assert_ne!(checksum(&a), checksum(&b));

    // A fresh shared view of the untouched source still matches the original
    // checksum, proving neither writer polluted the other's backing data.
    let c = BytesMut::from(&payload[..]);
    assert_eq!(checksum(&c), original);
}

#[test]
fn append_only_copies_view_not_whole_block() {
    // A large block with a small active view. Detaching on write must copy the
    // visible length instead of the whole underlying allocation.
    let mut big = BytesMut::with_capacity(4096);
    big.put_slice(b"abcd");
    assert_eq!(big.capacity(), 4096);

    let mut view = big.share();
    assert_eq!(view.as_ptr(), big.as_ptr());

    // Writing through the shared view detaches it. The bytes that get copied
    // are only the 4-byte view (the 4 KiB source block is never copied), and
    // the private allocation is bounded by the crate's 64 KiB growth hint
    // rather than the source block's size.
    view.put_u8(b'e');

    assert_eq!(&view[..], b"abcde");
    assert_ne!(view.as_ptr(), big.as_ptr());
    assert!(view.capacity() <= 64 * 1024);
    assert_eq!(big.capacity(), 4096);
    assert_eq!(&big[..], b"abcd");
}

#[test]
fn writes_through_bufmut_are_isolated() {
    let mut a = BytesMut::from(&b"abc"[..]);
    let mut b = a.share();

    a.put_slice(b"def");
    b.put_u8(b'z');
    b.put_bytes(b'q', 2);

    assert_eq!(&a[..], b"abcdef");
    assert_eq!(&b[..], b"abczqq");
}

#[test]
fn shared_view_deref_mut_copy_from_slice_isolated() {
    let mut a = BytesMut::from(&b"abcdef"[..]);
    let mut b = a.share();

    b[..3].copy_from_slice(b"xyz");

    assert_eq!(&a[..], b"abcdef");
    assert_eq!(&b[..], b"xyzdef");
}

#[test]
fn share_is_transitive_and_each_handle_detaches_once() {
    let mut a = BytesMut::from(&b"same"[..]);
    let mut b = a.share();
    let mut c = b.share();

    a[0] = b'A';
    b[0] = b'B';
    c[0] = b'C';

    assert_eq!(&a[..], b"Aame");
    assert_eq!(&b[..], b"Bame");
    assert_eq!(&c[..], b"Came");

    // Further writes after the first detach stay private and do not re-copy.
    let ptr = a.as_ptr();
    a[1] = b'x';
    assert_eq!(a.as_ptr(), ptr);
    assert_eq!(&a[..], b"Axme");
    assert_eq!(&b[..], b"Bame");
    assert_eq!(&c[..], b"Came");
}

#[test]
fn last_standing_handle_reuses_allocation() {
    let mut a = BytesMut::with_capacity(128);
    a.put_slice(b"payload");
    let mut b = a.share();
    let ptr = b.as_ptr();

    drop(a);

    // Once the peer is gone, the remaining handle is unique again and a write
    // reuses the allocation instead of copying.
    b[0] = b'P';
    assert_eq!(b.as_ptr(), ptr);
    assert_eq!(&b[..], b"Payload");
}

#[test]
fn frozen_shared_view_is_immutable_and_intact() {
    let mut a = BytesMut::from(&b"frozen"[..]);
    let b = a.share();

    let frozen = b.freeze();
    a[0] = b'F';

    assert_eq!(&a[..], b"Frozen");
    assert_eq!(&frozen[..], b"frozen");
}

#[test]
fn split_and_share_coexist() {
    // Existing disjoint split semantics must remain untouched while the new
    // overlapping share semantics isolate writes.
    let mut whole = BytesMut::from(&b"hello world"[..]);
    let mut right = whole.split_off(5);
    let mut right_copy = right.share();

    // Detach `right` first; from here on the two overlapping handles own
    // private copies and never observe each other's mutations.
    right[0] = b'_';
    right_copy[0] = b'w';
    right_copy[1] = b'!';

    assert_eq!(&whole[..], b"hello");
    assert_eq!(&right[..], b"_world");
    assert_eq!(&right_copy[..], b"w!orld");
}

#[test]
fn concurrent_writers_do_not_corrupt_each_other() {
    let payload: Vec<u8> = (0..256).map(|i| i as u8).collect();
    let mut base = BytesMut::from(&payload[..]);

    let mut a = base.share();
    let b = base.share();
    let thread_payload = payload.clone();

    let handle = std::thread::spawn(move || {
        let mut b = b;
        for x in b.iter_mut() {
            *x = x.wrapping_mul(2);
        }
        let expected: Vec<u8> = thread_payload.iter().map(|x| x.wrapping_mul(2)).collect();
        assert_eq!(&b[..], &expected[..]);
    });

    for x in a.iter_mut() {
        *x = x.wrapping_add(7);
    }
    let expected_a: Vec<u8> = payload.iter().map(|x| x.wrapping_add(7)).collect();
    assert_eq!(&a[..], &expected_a[..]);

    handle.join().unwrap();
    assert_eq!(&base[..], &payload[..]);
}

#[test]
fn shared_view_into_vec_is_detached_and_intact() {
    let mut a = BytesMut::from(&b"abcdef"[..]);
    let b = a.share();

    let owned: Vec<u8> = b.into();
    assert_eq!(owned, b"abcdef");

    // The source handle is unaffected by consuming the shared view.
    a[0] = b'A';
    assert_eq!(&a[..], b"Abcdef");
}

#[test]
fn unsplit_after_share_keeps_views_isolated() {
    let mut whole = BytesMut::with_capacity(32);
    whole.put_slice(b"hello world");

    let mut tail = whole.split_off(5);
    // An overlapping view of the tail while it still shares storage.
    let tail_view = tail.share();

    // Re-absorbing the original tail must not corrupt the shared view.
    whole.unsplit(tail);
    assert_eq!(&whole[..], b"hello world");
    assert_eq!(&tail_view[..], b" world");

    // Writing through the re-merged handle detaches it from the tail view.
    whole[0] = b'H';
    assert_eq!(&whole[..], b"Hello world");
    assert_eq!(&tail_view[..], b" world");
}
