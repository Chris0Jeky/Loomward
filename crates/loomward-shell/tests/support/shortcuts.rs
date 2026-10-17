pub fn put32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
pub fn header(flags: u32) -> Vec<u8> {
    let mut b = vec![0; 76];
    put32(&mut b, 0, 76);
    b[4..20].copy_from_slice(&[1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70]);
    put32(&mut b, 20, flags);
    put32(&mut b, 24, 32);
    b
}
pub fn finish(mut b: Vec<u8>) -> Vec<u8> {
    b.extend([0; 4]);
    b
}
pub fn string(b: &mut Vec<u8>, s: &str) {
    let u: Vec<u16> = s.encode_utf16().collect();
    b.extend((u.len() as u16).to_le_bytes());
    for c in u {
        b.extend(c.to_le_bytes());
    }
}
pub fn local_info() -> Vec<u8> {
    let mut b = vec![0; 28];
    put32(&mut b, 4, 28);
    put32(&mut b, 8, 1);
    put32(&mut b, 12, 28);
    b.extend(18u32.to_le_bytes());
    b.extend(3u32.to_le_bytes());
    b.extend(123u32.to_le_bytes());
    b.extend(16u32.to_le_bytes());
    b.extend(b"V\0");
    let at = b.len();
    put32(&mut b, 16, at as u32);
    b.extend(b"Q:\\fixture\0");
    let at = b.len();
    put32(&mut b, 24, at as u32);
    b.extend(b"\\doc.txt\0");
    let size = b.len();
    put32(&mut b, 0, size as u32);
    b
}

fn wide(b: &mut Vec<u8>, s: &str) {
    for c in s.encode_utf16().chain(Some(0)) {
        b.extend(c.to_le_bytes());
    }
}

pub fn network_info(unicode: bool) -> Vec<u8> {
    let mut n = vec![0; if unicode { 28 } else { 20 }];
    put32(&mut n, 4, 3);
    let at = n.len();
    put32(&mut n, 8, at as u32);
    put32(&mut n, 16, 0x20000);
    n.extend(b"\\\\server.invalid\\share\0");
    let at = n.len();
    put32(&mut n, 12, at as u32);
    n.extend(b"Z:\0");
    if unicode {
        let at = n.len();
        put32(&mut n, 20, at as u32);
        wide(&mut n, "\\\\server.invalid\\共有");
        let at = n.len();
        put32(&mut n, 24, at as u32);
        wide(&mut n, "Z:");
    }
    let size = n.len();
    put32(&mut n, 0, size as u32);
    let mut info = vec![0; 28];
    put32(&mut info, 4, 28);
    put32(&mut info, 8, 2);
    put32(&mut info, 20, 28);
    info.extend(n);
    let at = info.len();
    put32(&mut info, 24, at as u32);
    info.extend(b"\\doc.txt\0");
    let size = info.len();
    put32(&mut info, 0, size as u32);
    info
}

pub fn seeds(name: &str) -> [Vec<u8>; 9] {
    let mut local = header(2);
    local.extend(local_info());
    let mut text = header(0x84);
    string(&mut text, name);
    let mut ids = header(1);
    ids.extend(6u16.to_le_bytes());
    ids.extend([4, 0, 1, 2, 0, 0]);
    let mut environment = header(0x200);
    let mut block = vec![0; 788];
    put32(&mut block, 0, 788);
    put32(&mut block, 4, 0xa0000001);
    block[8..21].copy_from_slice(b"%ROOT%\\x.txt\0");
    environment.extend(block);
    let mut known = ids.clone();
    let mut block = vec![0; 28];
    put32(&mut block, 0, 28);
    put32(&mut block, 4, 0xa000000b);
    block[8..24].fill(7);
    known.extend(block);
    let mut tracker = header(0);
    let mut block = vec![0; 96];
    put32(&mut block, 0, 96);
    put32(&mut block, 4, 0xa0000003);
    put32(&mut block, 8, 88);
    block[16..23].copy_from_slice(b"fixture");
    block[32..64].fill(1);
    block[64..96].fill(2);
    tracker.extend(block);
    let network = |unicode| {
        let mut b = header(2);
        b.extend(network_info(unicode));
        finish(b)
    };
    [
        finish(header(0)),
        finish(local),
        finish(text),
        finish(ids),
        finish(environment),
        finish(known),
        finish(tracker),
        network(false),
        network(true),
    ]
}
