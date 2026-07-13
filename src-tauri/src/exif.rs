//! Container-level image metadata stripping on send (M9.1).
//!
//! WHAT this does: for JPEG/PNG/WebP it removes the EXIF, ICC-profile and XMP
//! blocks by editing the container's segment/chunk list — NOT by decoding the
//! image and re-encoding it. The compressed pixel data is copied through
//! byte-for-byte, so a stripped photo is visually identical to the original at
//! the bit level (no quality loss, no recompression). This is the whole reason
//! for `img-parts` over an image codec: privacy fields go, pixels stay.
//!
//! WHAT this deliberately does NOT do: any format `img-parts` cannot parse —
//! HEIC/HEIF, TIFF, GIF, camera RAW, SVG, videos — is passed through UNCHANGED
//! (the caller sends the original bytes). We do not claim to scrub what we
//! cannot open, and a corrupt or unusual file of a "supported" extension also
//! passes through rather than failing the send: stripping is a best-effort
//! privacy nicety, never a gate on delivering the user's file.
//!
//! Consistency contract (M9.1): whenever this returns cleaned bytes that the
//! caller decides to send, the manifest `size` AND `sha256` MUST be recomputed
//! over exactly those cleaned bytes — never the original — or the receiver's
//! hash check fails and the chunk stream desyncs. That recomputation lives in
//! `transfer::build_send_list_scoped`, which streams the cleaned bytes from a
//! temp file.

use img_parts::jpeg::Jpeg;
use img_parts::png::Png;
use img_parts::webp::WebP;
use img_parts::{Bytes, ImageEXIF, ImageICC};

/// Upper bound on how many bytes we will read into memory to attempt a strip.
/// Real photos — even 100-megapixel or panorama JPEGs — sit comfortably under
/// this; the cap only exists so a huge file that merely carries an image
/// extension (a video renamed `.jpg`, a multi-hundred-MB scan) is passed
/// through instead of being slurped whole into RAM on the sender. A file over
/// the cap is sent as-is (unstripped), never truncated.
pub const MAX_STRIP_BYTES: u64 = 256 * 1024 * 1024;

/// The JPEG APP1 marker (`0xFF 0xE1`, second byte). EXIF and XMP both live in
/// APP1 segments, distinguished only by their leading identifier — `img-parts`
/// removes EXIF via [`ImageEXIF::set_exif`] but has no XMP API, so we drop XMP
/// APP1 segments ourselves by their namespace prefix below.
const JPEG_APP1: u8 = 0xE1;

/// The identifier prefix of a JPEG XMP APP1 segment (`http://ns.adobe.com/xap/1.0/\0`).
/// An EXIF APP1 segment starts with `Exif\0\0` instead, so matching on this
/// prefix targets XMP alone and never touches the EXIF `set_exif` already handled.
const JPEG_XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The identifier prefix of a JPEG *Extended*-XMP APP1 segment
/// (`http://ns.adobe.com/xmp/extension/\0`). When an XMP packet exceeds the
/// ~64 KB a single APP1 segment can hold, encoders (notably Google/Samsung
/// phone cameras — Motion Photos, Ultra HDR gain-map and depth metadata) spill
/// the overflow into one or more Extended-XMP APP1 segments keyed by THIS
/// prefix, distinct from `JPEG_XMP_PREFIX`. They must be dropped too, or the
/// bulk of the XMP the user asked to scrub still ships with the photo.
const JPEG_XMP_EXT_PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// PNG chunk type carrying XMP (an `iTXt` chunk) and the XMP keyword that
/// identifies it. PNG EXIF (`eXIf`) and ICC (`iCCP`) are removed via the traits;
/// XMP has no trait, so we drop only `iTXt` chunks whose keyword is the XMP one.
const PNG_ITXT: [u8; 4] = *b"iTXt";
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp\0";

/// The WebP RIFF chunk id holding XMP metadata.
const WEBP_XMP_CHUNK: [u8; 4] = *b"XMP ";

/// Strip EXIF / ICC / XMP metadata from an in-memory image, WITHOUT touching the
/// pixel data (container-level segment/chunk removal only).
///
/// `ext` is the file extension (case-insensitive, no dot). Returns:
/// - `Some(cleaned)` when `ext` names a format we handle (JPEG/JPG, PNG, WebP),
///   the bytes parse as that container, and re-encoding succeeds. The result may
///   still equal the input when there was no metadata to remove — the caller
///   compares and only diverts to a temp copy when the bytes actually changed.
/// - `None` for any other extension, OR when the bytes fail to parse as the
///   claimed container (corrupt / mislabeled file). `None` means "pass the
///   original through unchanged" — stripping never fails the send.
///
/// Never panics on hostile input: every parse path returns `None` on error, and
/// re-encoding writes into an in-memory buffer that cannot fail.
pub fn strip_image_metadata(bytes: &[u8], ext: &str) -> Option<Vec<u8>> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => strip_jpeg(bytes),
        "png" => strip_png(bytes),
        "webp" => strip_webp(bytes),
        // Any other extension (HEIC/HEIF, TIFF, GIF, RAW, video, …) is a format
        // img-parts cannot open — pass it through untouched rather than pretend.
        _ => None,
    }
}

/// Whether `ext` is one [`strip_image_metadata`] can attempt. The caller uses
/// this to avoid reading a non-image file into memory just to have the strip
/// decline it — only genuinely strippable extensions are ever read.
pub fn is_strippable_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp"
    )
}

fn strip_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(bytes)).ok()?;
    // EXIF (APP1 `Exif\0\0`) and the ICC profile (APP2 `ICC_PROFILE\0`) have
    // dedicated removers.
    jpeg.set_exif(None);
    jpeg.set_icc_profile(None);
    // XMP has no img-parts API: it rides an APP1 segment keyed by the Adobe XMP
    // namespace, distinct from the EXIF APP1 just removed. Drop both the standard
    // XMP packet AND any Extended-XMP overflow segments (a >64 KB packet split
    // across several APP1s under a different prefix) so location/authorship XMP is
    // scrubbed in full, not just its first 64 KB.
    jpeg.segments_mut().retain(|s| {
        !(s.marker() == JPEG_APP1
            && (s.contents().starts_with(JPEG_XMP_PREFIX)
                || s.contents().starts_with(JPEG_XMP_EXT_PREFIX)))
    });
    Some(jpeg.encoder().bytes().to_vec())
}

fn strip_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut png = Png::from_bytes(Bytes::copy_from_slice(bytes)).ok()?;
    // EXIF (`eXIf`) and ICC (`iCCP`) via the traits.
    png.set_exif(None);
    png.set_icc_profile(None);
    // XMP rides an `iTXt` chunk whose keyword is the Adobe XMP one; remove only
    // those (leaving any other legitimate text chunks intact).
    png.chunks_mut()
        .retain(|c| !(c.kind() == PNG_ITXT && c.contents().starts_with(PNG_XMP_KEYWORD)));
    Some(png.encoder().bytes().to_vec())
}

fn strip_webp(bytes: &[u8]) -> Option<Vec<u8>> {
    use img_parts::webp::{CHUNK_EXIF, CHUNK_ICCP};
    let mut webp = WebP::from_bytes(Bytes::copy_from_slice(bytes)).ok()?;
    // Remove the metadata RIFF chunks DIRECTLY — do NOT route through
    // `set_exif(None)`/`set_icc_profile(None)`. Those call img-parts'
    // `convert_into_infered_kind()`, whose `infer_kind()` decides VP8X-vs-simple-VP8
    // by looking at ONLY the ICCP/EXIF chunks: once both are gone it concludes
    // "simple VP8" and DELETES the VP8X header — silently downgrading (corrupting)
    // any extended WebP that carries transparency (ALPH) or animation (ANIM/ANMF),
    // since those features REQUIRE VP8X. Plain `remove_chunks_by_id` only drops the
    // named chunk and leaves the VP8X container + ALPH/ANIM intact, so a
    // transparent sticker or animated WebP survives the strip losslessly.
    webp.remove_chunks_by_id(CHUNK_EXIF);
    webp.remove_chunks_by_id(CHUNK_ICCP);
    webp.remove_chunks_by_id(WEBP_XMP_CHUNK);
    Some(webp.encoder().bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble a tiny but structurally valid JPEG carrying an EXIF APP1
    /// segment plus a COM segment standing in for "image content", so a strip
    /// can be shown to drop the metadata while preserving the rest AND still
    /// round-tripping through img-parts' parser + encoder.
    ///
    /// Layout: SOI · APP1(`Exif\0\0` + payload) · COM("PIXELS") · SOS(header +
    /// entropy scan · EOI). WHY the SOS with entropy: img-parts attaches the
    /// trailing scan bytes (including the EOI) to the Start-Of-Scan segment and
    /// its encoder re-emits them there — a JPEG with no scan segment re-encodes
    /// without a terminating EOI and fails to re-parse. A real photo always has
    /// one; the fixture mirrors that so the round-trip is faithful.
    fn jpeg_with_exif() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8]; // SOI

        // APP1 EXIF: length covers the 2 length bytes + contents.
        let exif_contents: Vec<u8> = [b"Exif\x00\x00".as_slice(), &[0xAB; 24]].concat();
        out.push(0xFF);
        out.push(JPEG_APP1);
        out.extend_from_slice(&((exif_contents.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&exif_contents);

        // COM comment as a non-metadata payload that must survive the strip.
        let com = b"PIXELS".as_slice();
        out.push(0xFF);
        out.push(0xFE); // COM
        out.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(com);

        // SOS header (1-component minimal scan params) + opaque entropy + EOI.
        let sos_params = [0x01u8, 0x01, 0x00, 0x00, 0x3F, 0x00];
        out.push(0xFF);
        out.push(0xDA); // SOS
        out.extend_from_slice(&((sos_params.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&sos_params);
        out.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // entropy-coded scan bytes
        out.extend_from_slice(&[0xFF, 0xD9]); // EOI (carried with the scan)
        out
    }

    /// A JPEG with an injected EXIF segment comes back parseable, smaller, with
    /// the EXIF gone — and its non-metadata content preserved byte-for-byte.
    #[test]
    fn strips_exif_from_jpeg_preserving_content() {
        let input = jpeg_with_exif();
        // Sanity: the input really does carry EXIF.
        let before = Jpeg::from_bytes(Bytes::copy_from_slice(&input)).unwrap();
        assert!(before.exif().is_some(), "fixture must start with EXIF");

        let cleaned = strip_image_metadata(&input, "jpg").expect("jpeg is strippable");
        assert!(
            cleaned.len() < input.len(),
            "dropping the EXIF segment must shrink the file"
        );

        let after = Jpeg::from_bytes(Bytes::copy_from_slice(&cleaned))
            .expect("stripped bytes must still be a valid JPEG container");
        assert!(after.exif().is_none(), "EXIF must be gone after stripping");
        // The COM "PIXELS" content survives — non-metadata is untouched.
        assert!(
            cleaned.windows(6).any(|w| w == b"PIXELS"),
            "non-metadata content must be preserved"
        );
        // And no `Exif\0\0` marker lingers anywhere in the output.
        assert!(
            !cleaned.windows(6).any(|w| w == b"Exif\x00\x00"),
            "no EXIF identifier may remain"
        );
    }

    /// `jpg` and `jpeg` are treated identically (case-insensitive).
    #[test]
    fn jpeg_extension_is_case_and_spelling_insensitive() {
        let input = jpeg_with_exif();
        for ext in ["jpg", "JPG", "jpeg", "JPEG"] {
            assert!(
                strip_image_metadata(&input, ext).is_some(),
                "{ext} must be recognized as JPEG"
            );
        }
    }

    /// A non-image extension, and image bytes that fail to parse, both return
    /// `None` (pass the original through) rather than erroring.
    #[test]
    fn unknown_ext_or_corrupt_image_returns_none() {
        assert_eq!(strip_image_metadata(b"any bytes here", "txt"), None);
        assert_eq!(strip_image_metadata(b"", "heic"), None);
        // Claims to be a JPEG but is not — must pass through, never panic/fail.
        assert_eq!(strip_image_metadata(b"not really a jpeg", "jpg"), None);
        // Truncated JPEG signature.
        assert_eq!(strip_image_metadata(&[0xFF, 0xD8, 0x00], "jpeg"), None);
    }

    /// The extension gate reads exactly as the dispatcher: only the three
    /// handled formats are worth reading a file for.
    #[test]
    fn strippable_ext_matches_dispatch() {
        for e in ["jpg", "JPG", "jpeg", "png", "PNG", "webp", "WebP"] {
            assert!(is_strippable_ext(e), "{e} should be strippable");
        }
        for e in ["heic", "tiff", "gif", "cr2", "mp4", "txt", ""] {
            assert!(!is_strippable_ext(e), "{e} should not be strippable");
        }
    }

    // ── JPEG: Extended-XMP overflow (finding-1) ───────────────────────────

    /// Append a JPEG APP1 segment carrying `contents` (marker byte + big-endian
    /// length + contents), matching how `jpeg_with_exif` frames its segments.
    fn push_app1(out: &mut Vec<u8>, contents: &[u8]) {
        out.push(0xFF);
        out.push(JPEG_APP1);
        out.extend_from_slice(&((contents.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(contents);
    }

    /// A JPEG carrying an EXIF APP1, a standard XMP APP1, AND an Extended-XMP
    /// APP1 (the >64 KB overflow segment Google/Samsung phones split off).
    /// Mirrors `jpeg_with_exif`'s SOI · APP1(s) · COM · SOS(scan·EOI) skeleton so
    /// it round-trips img-parts' parser and encoder.
    fn jpeg_with_standard_and_extended_xmp() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8]; // SOI

        push_app1(
            &mut out,
            &[b"Exif\x00\x00".as_slice(), &[0xAB; 16]].concat(),
        );
        push_app1(
            &mut out,
            &[JPEG_XMP_PREFIX, b"<x:xmpmeta>std</x:xmpmeta>"].concat(),
        );
        push_app1(&mut out, &[JPEG_XMP_EXT_PREFIX, &[0xCD; 32]].concat());

        // COM content that must survive the strip.
        let com = b"PIXELS".as_slice();
        out.push(0xFF);
        out.push(0xFE); // COM
        out.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(com);

        // SOS header + entropy + EOI (see `jpeg_with_exif` for why the scan matters).
        let sos_params = [0x01u8, 0x01, 0x00, 0x00, 0x3F, 0x00];
        out.push(0xFF);
        out.push(0xDA); // SOS
        out.extend_from_slice(&((sos_params.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&sos_params);
        out.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // entropy-coded scan bytes
        out.extend_from_slice(&[0xFF, 0xD9]); // EOI
        out
    }

    /// The strip erases BOTH the standard XMP packet and the Extended-XMP
    /// overflow segment — so a photo with >64 KB of XMP is fully scrubbed, not
    /// just its first 64 KB. EXIF goes too; the COM content is preserved.
    #[test]
    fn strips_standard_and_extended_xmp_from_jpeg() {
        let input = jpeg_with_standard_and_extended_xmp();
        // Sanity: both XMP flavours really are present before the strip.
        assert!(
            input
                .windows(JPEG_XMP_PREFIX.len())
                .any(|w| w == JPEG_XMP_PREFIX),
            "fixture must carry standard XMP"
        );
        assert!(
            input
                .windows(JPEG_XMP_EXT_PREFIX.len())
                .any(|w| w == JPEG_XMP_EXT_PREFIX),
            "fixture must carry Extended-XMP"
        );

        let cleaned = strip_image_metadata(&input, "jpg").expect("jpeg is strippable");

        assert!(
            !cleaned
                .windows(JPEG_XMP_PREFIX.len())
                .any(|w| w == JPEG_XMP_PREFIX),
            "standard XMP must be gone"
        );
        assert!(
            !cleaned
                .windows(JPEG_XMP_EXT_PREFIX.len())
                .any(|w| w == JPEG_XMP_EXT_PREFIX),
            "Extended-XMP overflow must be gone"
        );
        assert!(
            !cleaned.windows(6).any(|w| w == b"Exif\x00\x00"),
            "EXIF must be gone too"
        );
        assert!(
            cleaned.windows(6).any(|w| w == b"PIXELS"),
            "non-metadata content must survive"
        );
    }

    // ── WebP: extended (VP8X) container integrity (finding-0) ──────────────

    /// Assemble a structurally valid RIFF/WebP from an ordered list of
    /// `(fourcc, payload)` chunks, computing the RIFF length and the per-chunk
    /// odd-length padding byte exactly as img-parts' parser expects. Lets a test
    /// build an *extended* (VP8X) WebP carrying ALPH/ANIM plus EXIF/XMP and prove
    /// the strip removes the metadata WITHOUT downgrading the container.
    fn webp_from_chunks(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WEBP");
        for (id, data) in chunks {
            body.extend_from_slice(*id);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            // RIFF pads an odd-length chunk payload to an even boundary.
            if data.len() % 2 == 1 {
                body.push(0x00);
            }
        }
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// A minimal 10-byte VP8X payload: flags byte + 3 reserved + canvas
    /// (width-1, height-1) as 24-bit LE (32x32 here). The flag bits are only
    /// illustrative — the strip path never reads them; the VP8X chunk's mere
    /// presence is what a transparent/animated WebP needs to stay valid.
    fn vp8x_payload(alpha: bool, anim: bool) -> [u8; 10] {
        let mut flags = 0x08u8; // 'E' EXIF-present (metadata lives in the fixture)
        if alpha {
            flags |= 0x10; // 'L' alpha
        }
        if anim {
            flags |= 0x02; // 'A' animation
        }
        let mut p = [0u8; 10];
        p[0] = flags;
        p[4] = 31; // canvas width-1  (low byte of 24-bit LE)
        p[7] = 31; // canvas height-1 (low byte of 24-bit LE)
        p
    }

    /// Stripping an extended (VP8X) WebP that carries TRANSPARENCY must remove
    /// EXIF/XMP but KEEP the VP8X header and the ALPH chunk. Routing through
    /// `set_exif(None)` would trip img-parts' `infer_kind` into deleting VP8X
    /// (downgrade to simple VP8), dropping the alpha channel and corrupting the
    /// image — while size+hash get recomputed over the broken bytes, hiding it.
    #[test]
    fn strip_webp_keeps_vp8x_and_alpha_dropping_only_metadata() {
        use img_parts::webp::{CHUNK_ALPH, CHUNK_EXIF, CHUNK_VP8, CHUNK_VP8X, CHUNK_XMP};
        let input = webp_from_chunks(&[
            (&CHUNK_VP8X, &vp8x_payload(true, false)),
            (&CHUNK_ALPH, &[0x00, 0x11, 0x22, 0x33]), // stand-in alpha data
            (&CHUNK_VP8, &[0xAA; 8]),                 // stand-in lossy bitstream
            (&CHUNK_EXIF, b"Exif\x00\x00\xDE\xAD"),
            (&CHUNK_XMP, b"<x:xmpmeta/>"),
        ]);

        // Sanity: the fixture is a VP8X WebP with alpha + metadata.
        let before = WebP::from_bytes(Bytes::copy_from_slice(&input)).unwrap();
        assert!(before.has_chunk(CHUNK_VP8X));
        assert!(before.has_chunk(CHUNK_ALPH));
        assert!(before.has_chunk(CHUNK_EXIF));

        let cleaned = strip_image_metadata(&input, "webp").expect("webp is strippable");
        assert!(
            cleaned.len() < input.len(),
            "removing EXIF+XMP must shrink the file"
        );

        let after = WebP::from_bytes(Bytes::copy_from_slice(&cleaned))
            .expect("stripped bytes must still be a valid WebP container");
        assert!(
            after.has_chunk(CHUNK_VP8X),
            "VP8X header must NOT be downgraded away"
        );
        assert!(
            after.has_chunk(CHUNK_ALPH),
            "the alpha (ALPH) chunk must survive"
        );
        assert!(
            after.has_chunk(CHUNK_VP8),
            "the pixel bitstream must survive"
        );
        assert!(!after.has_chunk(CHUNK_EXIF), "EXIF must be gone");
        assert!(!after.has_chunk(CHUNK_XMP), "XMP must be gone");
    }

    /// The same guarantee for an ANIMATED (VP8X + ANIM/ANMF) WebP: the strip
    /// keeps VP8X/ANIM/ANMF and removes only EXIF, so the animation still decodes.
    #[test]
    fn strip_webp_keeps_animation_chunks() {
        use img_parts::webp::{CHUNK_ANIM, CHUNK_ANMF, CHUNK_EXIF, CHUNK_VP8X};
        let input = webp_from_chunks(&[
            (&CHUNK_VP8X, &vp8x_payload(false, true)),
            (&CHUNK_ANIM, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]), // bg color + loop count
            (&CHUNK_ANMF, &[0x01; 20]),                           // stand-in frame
            (&CHUNK_EXIF, b"Exif\x00\x00\xBE\xEF"),
        ]);

        let cleaned = strip_image_metadata(&input, "webp").expect("webp is strippable");
        let after = WebP::from_bytes(Bytes::copy_from_slice(&cleaned)).unwrap();
        assert!(
            after.has_chunk(CHUNK_VP8X),
            "VP8X must survive on an animated WebP"
        );
        assert!(after.has_chunk(CHUNK_ANIM), "ANIM must survive");
        assert!(after.has_chunk(CHUNK_ANMF), "ANMF frame must survive");
        assert!(!after.has_chunk(CHUNK_EXIF), "EXIF must be gone");
    }

    /// A simple (non-extended) VP8 WebP with no metadata round-trips
    /// byte-for-byte — the strip removes nothing and never fabricates a VP8X.
    #[test]
    fn strip_webp_simple_vp8_passthrough_is_byte_identical() {
        use img_parts::webp::{CHUNK_VP8, CHUNK_VP8X};
        let input = webp_from_chunks(&[(&CHUNK_VP8, &[0x99; 10])]);
        let cleaned = strip_image_metadata(&input, "webp").expect("webp is strippable");
        assert_eq!(
            cleaned, input,
            "a clean simple-VP8 WebP must re-encode identically"
        );
        let after = WebP::from_bytes(Bytes::copy_from_slice(&cleaned)).unwrap();
        assert!(after.has_chunk(CHUNK_VP8), "the VP8 bitstream must remain");
        assert!(
            !after.has_chunk(CHUNK_VP8X),
            "no VP8X may be fabricated for a simple VP8"
        );
    }

    /// An odd-length VP8 payload exercises the RIFF even-boundary padding branch
    /// in `webp_from_chunks` — the fixture must still parse and round-trip.
    #[test]
    fn strip_webp_odd_length_chunk_padding_roundtrips() {
        use img_parts::webp::CHUNK_VP8;
        // 9 bytes is odd, so the fixture appends a pad byte to reach the RIFF
        // even boundary; a valid container must survive the strip unchanged.
        let input = webp_from_chunks(&[(&CHUNK_VP8, &[0x77; 9])]);
        let cleaned = strip_image_metadata(&input, "webp").expect("webp is strippable");
        assert_eq!(
            cleaned, input,
            "a clean odd-length simple-VP8 WebP must re-encode identically"
        );
        let after = WebP::from_bytes(Bytes::copy_from_slice(&cleaned)).unwrap();
        assert!(after.has_chunk(CHUNK_VP8), "the VP8 bitstream must remain");
    }

    // ── PNG: EXIF / ICC / XMP chunk removal ───────────────────────────────

    /// CRC-32 (IEEE, as PNG uses) over `data`, computed bitwise so the fixture
    /// stays self-contained (no dependency on a checksum crate).
    fn png_crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Append a PNG chunk: length (BE) · type · data · CRC-32 over type+data.
    fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&png_crc32(&crc_input).to_be_bytes());
    }

    /// Assemble a structurally valid PNG carrying an `eXIf` chunk and an `iTXt`
    /// XMP chunk (the two things the strip must drop), plus IHDR/IDAT/IEND that
    /// must survive. Not a decodable raster — img-parts only parses the chunk
    /// list — but a faithful container for the round-trip.
    fn png_with_exif_and_xmp() -> Vec<u8> {
        let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // signature
                                                                            // IHDR: 1x1, 8-bit, colour type 0 (greyscale), no interlace.
        let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0];
        push_png_chunk(&mut out, b"IHDR", &ihdr);
        // eXIf metadata chunk (dropped via set_exif).
        push_png_chunk(&mut out, b"eXIf", &[b'E', b'x', b'i', b'f', 0xDE, 0xAD]);
        // iTXt XMP: keyword (with its trailing NUL) + compression flag/method +
        // language + translated keyword + text (dropped by the XMP retain).
        let itxt = [PNG_XMP_KEYWORD, &[0, 0], b"\0", b"\0", b"<x:xmpmeta/>"].concat();
        push_png_chunk(&mut out, b"iTXt", &itxt);
        // IDAT stand-in pixel data that must be preserved.
        push_png_chunk(&mut out, b"IDAT", &[0x00, 0x01, 0x02, 0x03]);
        push_png_chunk(&mut out, b"IEND", &[]);
        out
    }

    /// A PNG carrying EXIF + XMP comes back parseable, smaller, with both gone
    /// while the IDAT pixel data survives.
    #[test]
    fn strips_exif_and_xmp_from_png() {
        let input = png_with_exif_and_xmp();
        // Sanity: the fixture parses and really carries EXIF.
        let before = Png::from_bytes(Bytes::copy_from_slice(&input)).unwrap();
        assert!(before.exif().is_some(), "fixture must start with EXIF");
        assert!(
            input
                .windows(PNG_XMP_KEYWORD.len())
                .any(|w| w == PNG_XMP_KEYWORD),
            "fixture must carry XMP"
        );

        let cleaned = strip_image_metadata(&input, "png").expect("png is strippable");
        assert!(
            cleaned.len() < input.len(),
            "dropping EXIF + XMP must shrink the file"
        );

        let after = Png::from_bytes(Bytes::copy_from_slice(&cleaned))
            .expect("stripped bytes must still be a valid PNG container");
        assert!(after.exif().is_none(), "EXIF must be gone after stripping");
        assert!(
            !cleaned
                .windows(PNG_XMP_KEYWORD.len())
                .any(|w| w == PNG_XMP_KEYWORD),
            "XMP iTXt chunk must be gone"
        );
        // IDAT pixel data survives — non-metadata is untouched.
        assert!(
            cleaned.windows(4).any(|w| w == b"IDAT"),
            "pixel data must be preserved"
        );
    }

    /// `png` is recognised case-insensitively, and a non-PNG byte string that
    /// merely claims the extension passes through as `None`.
    #[test]
    fn png_extension_case_insensitive_and_corrupt_passthrough() {
        let input = png_with_exif_and_xmp();
        for ext in ["png", "PNG", "Png"] {
            assert!(
                strip_image_metadata(&input, ext).is_some(),
                "{ext} must be recognized as PNG"
            );
        }
        assert_eq!(strip_image_metadata(b"not a png at all", "png"), None);
    }
}
