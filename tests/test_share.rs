#![warn(rust_2018_idioms)]

use bytes::{BufMut, BytesMut};

fn checksum(buf: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in buf {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn share_reads_same_memory_without_copy() {
    let mut a = BytesMut::from(&b"hello world"[..]);
    let b = a.share();

    assert_eq!(&a[..], &b[..]);
    assert_eq!(a.as_ptr(), b.as_ptr());
}

#[test]
fn writes_are_isolated_between_shared_views() {
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
fn concurrent_writers_keep_matching_checksums() {
    let payload: Vec<u8> = (0..=255).collect();
    let original_checksum = checksum(&payload);

    let mut a = BytesMut::from(&payload[..]);
    let mut b = a.share();
    let c = a.share();

    for byte in a.iter_mut() {
        *byte = byte.wrapping_add(1);
    }
    for byte in b.iter_mut() {
        *byte = byte.wrapping_sub(1);
    }

    let expected_a: Vec<u8> = payload.iter().map(|byte| byte.wrapping_add(1)).collect();
    let expected_b: Vec<u8> = payload.iter().map(|byte| byte.wrapping_sub(1)).collect();

    assert_eq!(&a[..], &expected_a[..]);
    assert_eq!(&b[..], &expected_b[..]);
    assert_eq!(checksum(&a), checksum(&expected_a));
    assert_eq!(checksum(&b), checksum(&expected_b));
    assert_eq!(&c[..], &payload[..]);
    assert_eq!(checksum(&c), original_checksum);
}

#[test]
fn frozen_snapshot_stays_fixed_while_writer_continues() {
    let mut writer = BytesMut::with_capacity(64);
    writer.put_slice(b"packet-1");
    let snapshot = writer.share().freeze();
    let sent_checksum = checksum(&snapshot);

    writer.clear();
    writer.put_slice(b"packet-2-with-more-data");

    assert_eq!(&snapshot[..], b"packet-1");
    assert_eq!(checksum(&snapshot), sent_checksum);
    assert_eq!(&writer[..], b"packet-2-with-more-data");
}

#[test]
fn detach_copies_only_the_visible_view() {
    let mut big = BytesMut::with_capacity(1 << 20);
    big.put_slice(b"abcd");
    let mut view = big.share();

    assert_eq!(view.as_ptr(), big.as_ptr());
    view.put_u8(b'e');

    assert_eq!(&view[..], b"abcde");
    assert_eq!(&big[..], b"abcd");
    assert_ne!(view.as_ptr(), big.as_ptr());
    assert!(view.capacity() <= 64 * 1024);
}

#[test]
fn all_mutation_paths_detach() {
    let mut a = BytesMut::from(&b"abc"[..]);
    let mut b = a.share();

    b.put_slice(b"def");
    b.put_u8(b'g');
    b.put_bytes(b'x', 2);
    b[..3].copy_from_slice(b"XYZ");

    assert_eq!(&a[..], b"abc");
    assert_eq!(&b[..], b"XYZdefgxx");
}

#[test]
fn last_handle_reuses_shared_allocation() {
    let mut a = BytesMut::with_capacity(128);
    a.put_slice(b"payload");
    let mut b = a.share();
    let ptr = b.as_ptr();

    drop(a);
    b[0] = b'P';

    assert_eq!(b.as_ptr(), ptr);
    assert_eq!(&b[..], b"Payload");
}

#[test]
fn shared_views_and_disjoint_splits_coexist() {
    let mut whole = BytesMut::from(&b"hello world"[..]);
    let mut right = whole.split_off(5);
    let mut right_view = right.share();

    right[0] = b'_';
    right_view[0] = b'w';
    right_view[1] = b'!';

    assert_eq!(&whole[..], b"hello");
    assert_eq!(&right[..], b"_world");
    assert_eq!(&right_view[..], b"w!orld");
}

#[test]
fn concurrent_shared_writers_do_not_interfere() {
    let payload: Vec<u8> = (0..256).map(|value| value as u8).collect();
    let mut base = BytesMut::from(&payload[..]);
    let mut a = base.share();
    let b = base.share();
    let thread_payload = payload.clone();

    let thread = std::thread::spawn(move || {
        let mut b = b;
        for byte in b.iter_mut() {
            *byte = byte.wrapping_mul(2);
        }
        assert_eq!(
            &b[..],
            &thread_payload
                .iter()
                .map(|byte| byte.wrapping_mul(2))
                .collect::<Vec<_>>()[..]
        );
    });

    for byte in a.iter_mut() {
        *byte = byte.wrapping_add(7);
    }

    assert_eq!(
        &a[..],
        &payload[..]
            .iter()
            .map(|byte| byte.wrapping_add(7))
            .collect::<Vec<_>>()[..]
    );
    thread.join().unwrap();
    assert_eq!(&base[..], &payload[..]);
}
