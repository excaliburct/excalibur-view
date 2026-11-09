//! Stream filters.
//!
//! Streams arrive encoded and are decoded on demand. Cross reference streams in
//! particular are almost always Flate with a PNG predictor, so this has to be
//! right before a single page can be found.

use crate::object::{Dict, Object, Stream};

#[derive(Debug)]
pub enum Trouble {
    Unsupported(String),
    Corrupt(String),
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Unsupported(what) => write!(f, "{what} streams are not supported"),
            Trouble::Corrupt(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Trouble {}

/// Filters whose output is an image rather than bytes we can use. These are
/// left encoded; the renderer deals with them.
pub fn is_image_filter(name: &str) -> bool {
    matches!(
        name,
        "DCTDecode" | "JPXDecode" | "JBIG2Decode" | "CCITTFaxDecode" | "DCT" | "CCF"
    )
}

fn filter_names(dict: &Dict) -> Vec<String> {
    let entry = dict.get("Filter").or_else(|| dict.get("F"));
    match entry {
        Some(Object::Name(n)) => vec![n.as_str().to_string()],
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(|o| o.as_name().map(|n| n.as_str().to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn decode_parms(dict: &Dict, count: usize) -> Vec<Option<Dict>> {
    let entry = dict
        .get("DecodeParms")
        .or_else(|| dict.get("DP"))
        .or_else(|| dict.get("DecodeParams"));
    match entry {
        Some(Object::Dict(d)) => {
            let mut v = vec![None; count];
            if count > 0 {
                v[0] = Some(d.clone());
            }
            v
        }
        Some(Object::Array(a)) => (0..count)
            .map(|i| a.get(i).and_then(|o| o.as_dict()).cloned())
            .collect(),
        _ => vec![None; count],
    }
}

/// The most one stream may decode to. A few hundred bytes of nested Flate
/// can claim terabytes; a drawing's real streams come to a few megabytes,
/// and the biggest content streams in the wild to tens. Past this a stream
/// is refused as damaged rather than read until the computer runs out of
/// memory, which on the office server stops it for everybody.
pub const LARGEST_DECODED: usize = 256 * 1024 * 1024; // WHAT: 256 MiB. WHY: well past any real stream, well short of a server's memory.

/// Decodes a stream's bytes. Image filters are left alone and reported through
/// `still_encoded`, because the caller wants those bytes as they are.
pub fn decode(stream: &Stream) -> std::result::Result<Vec<u8>, Trouble> {
    decode_within(stream, LARGEST_DECODED) // WHAT: decoded, to the usual limit. WHY: every caller gets the same protection.
}

/// `decode`, refusing a stream that decodes to more than `limit` bytes at
/// any step.
///
/// # Errors
///
/// A filter that can't be decoded, one this reader doesn't know, or more than
/// `limit` bytes decoded at any step.
pub fn decode_within(stream: &Stream, limit: usize) -> std::result::Result<Vec<u8>, Trouble> {
    let (data, remaining) = decode_to_within(stream, limit)?;
    match remaining.first() {
        Some(name) => Err(Trouble::Unsupported(name.clone())),
        None => Ok(data),
    }
}

/// Like `decode`, but stops at the first image filter and reports what is left.
pub fn decode_to(stream: &Stream) -> std::result::Result<(Vec<u8>, Vec<String>), Trouble> {
    decode_to_within(stream, LARGEST_DECODED) // WHAT: to the usual limit. WHY: as `decode`.
}

/// The most filters one stream may name. Real files name one, now and then
/// two or three.
pub const LONGEST_CHAIN: usize = 8; // WHAT: eight. WHY: well past any real file; each filter is a pass over the whole stream, so the file must not choose how many.

fn decode_to_within(
    stream: &Stream,
    limit: usize,
) -> std::result::Result<(Vec<u8>, Vec<String>), Trouble> {
    let names = filter_names(&stream.dict);
    if names.len() > LONGEST_CHAIN {
        return Err(Trouble::Corrupt(format!(
            "a stream names {} filters, which no drawing does",
            names.len()
        ))); // WHAT: refused as damage before any is run. WHY: forty thousand filters over a few megabytes was a hundred gigabytes of copying.
    }
    let parms = decode_parms(&stream.dict, names.len());
    let mut data = stream.data.clone();
    for (i, name) in names.iter().enumerate() {
        if is_image_filter(name) {
            return Ok((data, names[i..].to_vec()));
        }
        let parm = parms.get(i).cloned().flatten();
        data = one(name, &data, parm.as_ref(), limit)?; // WHAT: one filter, held to the limit. WHY: each step of a chain is a chance to multiply.
    }
    Ok((data, Vec::new()))
}

/// The refusal for a stream that decodes to more than `limit`.
fn too_big(limit: usize) -> Trouble {
    Trouble::Corrupt(format!(
        "a stream decodes to more than {} MiB, which no drawing needs",
        limit / (1024 * 1024)
    )) // WHAT: said as damage. WHY: treated as a broken stream, which every caller already handles.
}

fn one(
    name: &str,
    data: &[u8],
    parm: Option<&Dict>,
    limit: usize,
) -> std::result::Result<Vec<u8>, Trouble> {
    let out = match name {
        "FlateDecode" | "Fl" => inflate(data, limit)?,
        "LZWDecode" | "LZW" => {
            let early = parm
                .and_then(|p| p.get("EarlyChange"))
                .and_then(|o| o.as_i64())
                .unwrap_or(1);
            lzw(data, early != 0, limit)?
        }
        "ASCIIHexDecode" | "AHx" => ascii_hex(data),
        "ASCII85Decode" | "A85" => ascii_85(data),
        "RunLengthDecode" | "RL" => run_length(data, limit)?,
        "Crypt" => data.to_vec(),
        other => return Err(Trouble::Unsupported(other.to_string())),
    };
    Ok(match parm {
        Some(p) => unpredict(out, p),
        None => out,
    })
}

/// Zlib, falling back to raw deflate and then to whatever decompressed before
/// the stream went bad. Truncated streams are common in files that have been
/// through several programs, and half a content stream beats none.
fn inflate(data: &[u8], limit: usize) -> std::result::Result<Vec<u8>, Trouble> {
    use flate2::read::{DeflateDecoder, ZlibDecoder};

    let start = data
        .iter()
        .position(|b| !crate::parse::is_space(*b))
        .unwrap_or(0);
    let data = &data[start..];
    if data.is_empty() {
        return Ok(Vec::new());
    }

    let out = read_what_we_can(ZlibDecoder::new(data), limit)?; // WHAT: zlib, no further than the limit. WHY: a bomb is refused, not followed.
    if !out.is_empty() {
        return Ok(out);
    }
    // A few writers omit the zlib header and hand over raw deflate.
    let out = read_what_we_can(DeflateDecoder::new(data), limit)?; // WHAT: raw deflate, held the same. WHY: the same.
    if !out.is_empty() {
        return Ok(out);
    }
    Err(Trouble::Corrupt(
        "compressed stream is damaged beyond reading".into(),
    ))
}

/// Reads until the reader stops or breaks, keeping whatever came out first.
/// `read_to_end` throws away partial output on error, and partial output is
/// exactly what a half-written drawing set has to offer.
fn read_what_we_can(
    mut reader: impl std::io::Read,
    limit: usize,
) -> std::result::Result<Vec<u8>, Trouble> {
    let mut out = Vec::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() + n > limit {
                    return Err(too_big(limit)); // WHAT: past the limit. WHY: stopped here, before the memory goes.
                }
                out.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Ok(out)
}

/// Where an LZW stream is written: refuses to grow past its limit.
struct Capped {
    out: Vec<u8>, // WHAT: what has come out. WHY: handed back when whole.
    limit: usize, // WHAT: the most it may hold. WHY: as for Flate.
    over: bool,   // WHAT: whether the limit was hit. WHY: told apart from a damaged stream.
}

impl std::io::Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.out.len() + bytes.len() > self.limit {
            self.over = true; // WHAT: noted. WHY: said as too big, not as damaged.
            return Err(std::io::Error::other("too big")); // WHAT: no more taken. WHY: the decoder stops.
        }
        self.out.extend_from_slice(bytes); // WHAT: kept. WHY: within the limit.
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(()) // WHAT: nothing to flush. WHY: it is memory.
    }
}

fn lzw(data: &[u8], early_change: bool, limit: usize) -> std::result::Result<Vec<u8>, Trouble> {
    use weezl::decode::Decoder;
    use weezl::BitOrder;
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    let mut capped = Capped {
        out: Vec::new(),
        limit,
        over: false,
    }; // WHAT: somewhere to write that refuses to grow too far. WHY: LZW multiplies as Flate does.
    let result = decoder.into_stream(&mut capped).decode_all(data); // WHAT: decoded into it. WHY: stops at the limit.
    if capped.over {
        return Err(too_big(limit)); // WHAT: refused for its size. WHY: as Flate.
    }
    result
        .status
        .map_err(|e| Trouble::Corrupt(format!("LZW stream is damaged: {e}")))?; // WHAT: damage said as before. WHY: the same refusal as ever.
    Ok(capped.out)
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut half: Option<u8> = None;
    for b in data {
        if *b == b'>' {
            break;
        }
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => continue,
        };
        match half.take() {
            Some(hi) => out.push(hi * 16 + v),
            None => half = Some(v),
        }
    }
    if let Some(hi) = half {
        out.push(hi * 16);
    }
    out
}

fn ascii_85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut n = 0usize;
    let mut i = 0usize;
    // The optional <~ opener.
    if data.starts_with(b"<~") {
        i = 2;
    }
    while i < data.len() {
        let b = data[i];
        i += 1;
        if b == b'~' {
            break;
        }
        if crate::parse::is_space(b) {
            continue;
        }
        if b == b'z' && n == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            continue;
        }
        group[n] = b - b'!';
        n += 1;
        if n == 5 {
            let mut value: u32 = 0;
            for g in group {
                value = value.wrapping_mul(85).wrapping_add(g as u32);
            }
            out.extend_from_slice(&value.to_be_bytes());
            n = 0;
        }
    }
    if n > 0 {
        // A partial final group is padded with 'u' and then trimmed.
        for slot in group.iter_mut().skip(n) {
            *slot = 84;
        }
        let mut value: u32 = 0;
        for g in group {
            value = value.wrapping_mul(85).wrapping_add(g as u32);
        }
        let bytes = value.to_be_bytes();
        out.extend_from_slice(&bytes[..n - 1]);
    }
    out
}

fn run_length(data: &[u8], limit: usize) -> std::result::Result<Vec<u8>, Trouble> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        if out.len() > limit {
            return Err(too_big(limit)); // WHAT: past the limit. WHY: a run of 128 copies per two bytes multiplies sixty-four times.
        }
        let n = data[i];
        i += 1;
        match n {
            128 => break,
            0..=127 => {
                let count = n as usize + 1;
                let end = (i + count).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            _ => {
                if i < data.len() {
                    let count = 257 - n as usize;
                    out.extend(std::iter::repeat(data[i]).take(count));
                    i += 1;
                }
            }
        }
    }
    Ok(out)
}

/// Reverses the PNG and TIFF predictors that compressors apply before deflate.
fn unpredict(data: Vec<u8>, parm: &Dict) -> Vec<u8> {
    let predictor = parm.get("Predictor").and_then(Object::as_i64).unwrap_or(1);
    if predictor <= 1 {
        return data;
    }
    // WHAT: a predictor number from the file, at least 1, as a size; one too big for this machine's sizes becomes the largest. WHY: the largest then fails the checked multiply below and the bytes pass on as they are, where a cast would have cut it to some other number.
    let size = |key: &str, default: i64| {
        usize::try_from(
            parm.get(key)
                .and_then(Object::as_i64)
                .unwrap_or(default)
                .max(1),
        )
        .unwrap_or(usize::MAX)
    };
    let colors = size("Colors", 1); // WHAT: colour components per sample. WHY: part of the row's length.
    let bpc = size("BitsPerComponent", 8); // WHAT: bits per component. WHY: the same.
    let columns = size("Columns", 1); // WHAT: samples per row. WHY: the same.

    // WHAT: the row's length worked out without overflowing, and never longer than the data. WHY: the numbers are the file's; a row of a terabyte asked for a terabyte of memory and closed the program.
    let Some(bits) = columns.checked_mul(colors).and_then(|n| n.checked_mul(bpc)) else {
        return data; // WHAT: numbers too big to multiply. WHY: no real predictor has them; the bytes are passed on as they are.
    };
    let row = bits.div_ceil(8).clamp(1, data.len().max(1)); // WHAT: at most the whole of the data. WHY: a row longer than the stream can't be one.
    let sample = (colors.saturating_mul(bpc)).div_ceil(8).clamp(1, row); // WHAT: one sample's bytes, within a row. WHY: the same.

    if predictor == 2 {
        // TIFF predictor: each sample is a delta from the one before it.
        if bpc != 8 {
            return data;
        }
        let mut data = data;
        for line in data.chunks_mut(row) {
            for i in sample..line.len() {
                line[i] = line[i].wrapping_add(line[i - sample]);
            }
        }
        return data;
    }

    // PNG predictors: every row carries a leading filter byte.
    let stride = row + 1;
    let mut out = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; row];
    let mut at = 0usize;
    while at + 1 <= data.len() {
        let kind = data[at];
        let start = at + 1;
        let end = (start + row).min(data.len());
        if start >= data.len() {
            break;
        }
        let mut line = data[start..end].to_vec();
        line.resize(row, 0);
        for i in 0..row {
            let left = if i >= sample { line[i - sample] } else { 0 };
            let up = previous[i];
            let up_left = if i >= sample { previous[i - sample] } else { 0 };
            line[i] = match kind {
                0 => line[i],
                1 => line[i].wrapping_add(left),
                2 => line[i].wrapping_add(up),
                3 => line[i].wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => line[i].wrapping_add(paeth(left, up, up_left)),
                _ => line[i],
            };
        }
        out.extend_from_slice(&line);
        previous = line;
        at += stride;
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Compresses with zlib, for objects Hyperview writes.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;
    use crate::object::Object;

    fn stream(dict: Dict, data: Vec<u8>) -> Stream {
        Stream { dict, data }
    }

    #[test]
    fn flate_round_trips() {
        let text = b"W12x26 at 26.00 lb per foot".repeat(40);
        let s = stream(
            dict! {"Filter" => Object::name("FlateDecode")},
            deflate(&text),
        );
        assert_eq!(decode(&s).unwrap(), text);
    }

    #[test]
    fn a_truncated_flate_stream_gives_back_what_it_managed() {
        let text = b"the quick brown fox jumps over the lazy dog".repeat(50);
        let full = deflate(&text);
        let s = stream(
            dict! {"Filter" => Object::name("FlateDecode")},
            full[..full.len() / 2].to_vec(),
        );
        let out = decode(&s).unwrap();
        assert!(!out.is_empty(), "half a stream beats none");
        assert!(
            text.starts_with(&out),
            "what came out has to be a prefix of what went in, not guesswork"
        );
    }

    #[test]
    fn ascii_hex_and_ascii85_decode() {
        let s = stream(
            dict! {"Filter" => Object::name("ASCIIHexDecode")},
            b"48656C6C6F>".to_vec(),
        );
        assert_eq!(decode(&s).unwrap(), b"Hello");

        let s = stream(
            dict! {"Filter" => Object::name("ASCII85Decode")},
            b"87cURD]i,\"Ebo80~>".to_vec(),
        );
        assert_eq!(decode(&s).unwrap(), b"Hello World!");
    }

    const _: () = assert!(
        LARGEST_DECODED <= 256 * 1024 * 1024,
        "the everyday limit stays a sane size"
    ); // WHAT: checked when the tests are built. WHY: the limit can't creep up unnoticed.

    #[test]
    fn a_stream_that_decodes_past_its_limit_is_refused_rather_than_followed() {
        // WHAT: Flate of a mebibyte of zeros, once and twice over, with a limit below it. WHY: nested Flate turns a few hundred bytes into terabytes; it must stop at the limit.
        let big = vec![0u8; 1024 * 1024];
        let once = stream(
            dict! {"Filter" => Object::name("FlateDecode")},
            deflate(&big),
        );
        let twice = stream(
            dict! {"Filter" => Object::Array(vec![Object::name("FlateDecode"), Object::name("FlateDecode")])},
            deflate(&deflate(&big)),
        );
        for s in [&once, &twice] {
            assert!(
                decode_within(s, 1024 * 1024 - 1).is_err(),
                "one byte over the limit is refused"
            );
            assert_eq!(
                decode_within(s, 1024 * 1024).unwrap().len(),
                big.len(),
                "exactly the limit is read whole"
            );
        }
        // WHAT: run length's copies held the same. WHY: two bytes make 128.
        let runs = stream(
            dict! {"Filter" => Object::name("RunLengthDecode")},
            [129u8, 0].repeat(10_000),
        );
        assert!(decode_within(&runs, 1000).is_err());
        // WHAT: LZW too. WHY: it multiplies as well.
        let lzw_bytes = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&big)
            .unwrap();
        let lzw = stream(dict! {"Filter" => Object::name("LZWDecode")}, lzw_bytes);
        assert!(decode_within(&lzw, 1000).is_err());
        assert_eq!(decode_within(&lzw, big.len()).unwrap().len(), big.len());
    }

    #[test]
    fn a_chain_of_filters_longer_than_any_real_one_is_refused_before_it_is_run() {
        // WHAT: four megabytes put through forty thousand Crypt filters. WHY: each one copied the whole stream, so a few hundred kilobytes of names made the reader copy 160 GB.
        let names: Vec<Object> = (0..40_000).map(|_| Object::name("Crypt")).collect();
        let s = stream(
            dict! {"Filter" => Object::Array(names)},
            vec![7u8; 4 * 1024 * 1024],
        );
        let started = std::time::Instant::now(); // WHAT: now. WHY: the refusal must be at once.
        assert!(
            decode(&s).is_err(),
            "a chain of forty thousand filters was run"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        let hexed: String = deflate(b"hello")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(); // WHAT: "hello", deflated, written as hex. WHY: a real two-filter chain.
        let ordinary = stream(
            dict! {"Filter" => Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("FlateDecode")])},
            format!("{hexed}>").into_bytes(),
        ); // WHAT: the chain. WHY: still read.
        assert_eq!(
            decode(&ordinary).unwrap(),
            b"hello",
            "an ordinary chain still reads"
        );
    }

    #[test]
    fn a_predictor_whose_numbers_are_absurd_asks_for_no_absurd_memory() {
        // WHAT: a PNG predictor claiming rows of a terabyte, and one whose numbers overflow. WHY: the row's size was allocated as asked, which closed the program.
        for columns in [1i64 << 40, i64::MAX] {
            let parms = dict! {"Predictor" => Object::Int(12), "Columns" => Object::Int(columns), "Colors" => Object::Int(4)};
            let s = stream(
                dict! {"Filter" => Object::name("FlateDecode"), "DecodeParms" => Object::Dict(parms)},
                deflate(&[2u8, 1, 2, 3, 4, 5]),
            );
            let out = decode(&s).unwrap(); // WHAT: decoded at all. WHY: came back without a terabyte.
            assert!(out.len() <= 6, "{} bytes from six", out.len());
        }
    }

    #[test]
    fn run_length_decodes_runs_and_literals() {
        // 2 -> three literals; 254 -> three copies; 128 -> stop.
        let s = stream(
            dict! {"Filter" => Object::name("RunLengthDecode")},
            vec![2, b'a', b'b', b'c', 254, b'z', 128],
        );
        assert_eq!(decode(&s).unwrap(), b"abczzz");
    }

    #[test]
    fn filters_apply_in_order() {
        let inner = deflate(b"nested");
        let hexed: String = inner.iter().map(|b| format!("{b:02X}")).collect();
        let s = stream(
            dict! {"Filter" => Object::Array(vec![
                Object::name("ASCIIHexDecode"),
                Object::name("FlateDecode"),
            ])},
            format!("{hexed}>").into_bytes(),
        );
        assert_eq!(decode(&s).unwrap(), b"nested");
    }

    #[test]
    fn the_png_up_predictor_is_reversed() {
        // Two rows of three bytes, second row filtered as "up".
        let raw = vec![0, 10, 20, 30, 2, 1, 1, 1];
        let s = stream(
            dict! {
                "Filter" => Object::name("FlateDecode"),
                "DecodeParms" => Object::Dict(dict!{
                    "Predictor" => Object::Int(12),
                    "Columns" => Object::Int(3),
                }),
            },
            deflate(&raw),
        );
        assert_eq!(decode(&s).unwrap(), vec![10, 20, 30, 11, 21, 31]);
    }

    #[test]
    fn an_image_stream_is_handed_back_still_encoded() {
        let s = stream(
            dict! {"Filter" => Object::name("DCTDecode")},
            vec![0xff, 0xd8, 0xff],
        );
        let (data, left) = decode_to(&s).unwrap();
        assert_eq!(left, vec!["DCTDecode".to_string()]);
        assert_eq!(data, vec![0xff, 0xd8, 0xff]);
        assert!(decode(&s).is_err());
    }
}
