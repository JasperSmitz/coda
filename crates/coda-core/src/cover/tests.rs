use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

use super::cache::{self, DiskCache};
use super::*;
use crate::model::TrackMetadata;

fn pixels(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, 128])
    })
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            &pixels(width, height),
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    out
}

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode(
            &pixels(width, height),
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    out
}

fn webp(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(
            &pixels(width, height),
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    out
}

fn solid(width: u32, height: u32, rgb: [u8; 3]) -> CoverImage {
    CoverImage::new(width, height, rgb.repeat((width * height) as usize)).unwrap()
}

#[test]
fn formats_are_recognised_by_their_first_bytes() {
    assert_eq!(sniff(&jpeg(4, 4)), Some(Format::Jpeg));
    assert_eq!(sniff(&png(4, 4)), Some(Format::Png));
    assert_eq!(sniff(&webp(4, 4)), Some(Format::WebP));
    assert_eq!(sniff(b"GIF89a......"), None);
    assert_eq!(sniff(br#"{"subsonic-response":{}}"#), None);
    assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None, "a WAV is RIFF too");
    assert_eq!(sniff(&[]), None);
    assert_eq!(sniff(&[0xFF, 0xD8]), None);
}

#[test]
fn jpeg_png_and_webp_decode_to_square_covers() {
    for bytes in [jpeg(40, 40), png(40, 40), webp(40, 40)] {
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (40, 40));
        assert_eq!(image.rgb().len(), 40 * 40 * 3);
    }
    // Larger covers come down to `COVER_SIZE`; a wide one is cropped to
    // its centre first.
    let image = decode(&png(600, 600)).unwrap();
    assert_eq!((image.width(), image.height()), (COVER_SIZE, COVER_SIZE));
    let wide = decode(&png(300, 100)).unwrap();
    assert_eq!((wide.width(), wide.height()), (100, 100));
    // The centre: the gradient's red at the left edge is about a third.
    let [r, _, _] = wide.pixel(0, 50);
    assert!((80..=90).contains(&r), "{r}");
}

#[test]
fn covers_decode_at_the_large_size_within_the_same_limits() {
    let image = decode_at(&jpeg(1500, 1200), LARGE_COVER_SIZE).unwrap();
    assert_eq!(
        (image.width(), image.height()),
        (LARGE_COVER_SIZE, LARGE_COVER_SIZE)
    );
    // A source smaller than the size is kept as it is.
    let image = decode_at(&png(600, 600), LARGE_COVER_SIZE).unwrap();
    assert_eq!(image.width(), 600);
    assert_eq!(
        decode_at(&png(MAX_DIMENSION + 1, 1), LARGE_COVER_SIZE),
        Err(DecodeError::TooLarge)
    );
    assert_eq!(CoverSize::Small.pixels(), COVER_SIZE);
    assert_eq!(CoverSize::Large.pixels(), LARGE_COVER_SIZE);
}

#[test]
fn hostile_images_are_refused_without_allocating() {
    assert_eq!(decode(b"not an image"), Err(DecodeError::NotAnImage));
    assert_eq!(decode(&[]), Err(DecodeError::NotAnImage));
    // A JPEG whose header claims 65000×65000 pixels: refused by the limits
    // before the 12 GB it would take is allocated.
    let mut bomb = jpeg(16, 16);
    let sof = bomb.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    bomb[sof + 5..sof + 7].copy_from_slice(&65000u16.to_be_bytes());
    bomb[sof + 7..sof + 9].copy_from_slice(&65000u16.to_be_bytes());
    let started = std::time::Instant::now();
    assert_eq!(decode(&bomb), Err(DecodeError::TooLarge));
    assert!(started.elapsed() < Duration::from_secs(1));
    // Over the dimension limit, however small the file.
    assert_eq!(
        decode(&png(MAX_DIMENSION + 1, 1)),
        Err(DecodeError::TooLarge)
    );
    // Truncated and corrupt data.
    let full = jpeg(64, 64);
    assert!(decode(&full[..full.len() / 3]).is_err());
    let mut corrupt = png(8, 8);
    corrupt.truncate(20);
    assert_eq!(decode(&corrupt), Err(DecodeError::Malformed));
    // More bytes than any cover needs.
    let mut huge = jpeg(4, 4);
    huge.resize(MAX_IMAGE_BYTES + 1, 0);
    assert_eq!(decode(&huge), Err(DecodeError::TooLarge));
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend(data);
    out.extend(&body);
    out.extend(crc32(&body).to_be_bytes());
}

/// A PNG header claiming `side`² 16-bit RGBA pixels, with almost no data.
fn png_bomb(side: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(side.to_be_bytes());
    header.extend(side.to_be_bytes());
    header.extend([16, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &header);
    // A zlib stream of one empty stored block.
    png_chunk(
        &mut out,
        b"IDAT",
        &[
            0x78, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01,
        ],
    );
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// A lossless WebP header claiming `side`² pixels, with no image data.
fn webp_bomb(side: u32) -> Vec<u8> {
    let bits = (side - 1) | ((side - 1) << 14) | (1 << 28);
    let mut vp8l = vec![0x2F];
    vp8l.extend(bits.to_le_bytes());
    vp8l.extend([0u8; 16]);
    let mut out = b"RIFF".to_vec();
    out.extend((4 + 8 + vp8l.len() as u32).to_le_bytes());
    out.extend(b"WEBPVP8L");
    out.extend((vp8l.len() as u32).to_le_bytes());
    out.extend(vp8l);
    out
}

#[test]
fn bombs_within_the_dimension_limit_are_refused_quickly() {
    let started = std::time::Instant::now();
    // 4096² × 8 bytes = 128 MiB of output: over the allocation limit.
    assert_eq!(decode(&png_bomb(MAX_DIMENSION)), Err(DecodeError::TooLarge));
    // A WebP claiming 10000²: over the
    // dimension limit; one within it but with no data: malformed.
    assert_eq!(decode(&webp_bomb(10_000)), Err(DecodeError::TooLarge));
    assert!(decode(&webp_bomb(MAX_DIMENSION)).is_err());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn resizing_averages_what_each_pixel_covers() {
    // Left half black, right half white.
    let mut rgb = Vec::new();
    for _ in 0..4 {
        rgb.extend([0, 0, 0].repeat(2));
        rgb.extend([255, 255, 255].repeat(2));
    }
    let image = CoverImage::new(4, 4, rgb).unwrap();
    let small = image.resized(2, 1);
    assert_eq!((small.width(), small.height()), (2, 1));
    assert_eq!(small.pixel(0, 0), [0, 0, 0]);
    assert_eq!(small.pixel(1, 0), [255, 255, 255]);
    let one = image.resized(1, 1);
    assert_eq!(one.pixel(0, 0), [127, 127, 127]);
    // Enlarging and odd sizes never index out of range.
    let big = image.resized(9, 7);
    assert_eq!((big.width(), big.height()), (9, 7));
    assert_eq!(big.pixel(0, 0), [0, 0, 0]);
    assert_eq!(big.pixel(8, 6), [255, 255, 255]);
    assert_eq!(image.resized(0, 0).width(), 1);
    assert!(CoverImage::new(2, 2, vec![0; 11]).is_none());
    assert!(CoverImage::new(0, 2, Vec::new()).is_none());
}

// ----- the cover state ------------------------------------------------

fn local_track(path: &str) -> Track {
    Track::local(path.into(), TrackMetadata::default())
}

fn remote_track(song: &str, cover: Option<&str>) -> Track {
    Track::navidrome(
        song.into(),
        TrackMetadata {
            cover_art: cover.map(str::to_owned),
            ..TrackMetadata::default()
        },
    )
}

fn answer(request: &CoverRequest, outcome: Outcome) -> CoverResponse {
    CoverResponse {
        id: request.id,
        generation: request.generation,
        key: request.key.clone(),
        size: request.size,
        outcome,
    }
}

fn art() -> Outcome {
    Outcome::Art(Arc::new(solid(2, 2, [9, 9, 9])))
}

#[test]
fn the_cover_follows_the_track_and_shows_only_its_own_result() {
    let mut covers = Covers::new(true);
    let a = local_track("/m/a.flac");
    let b = local_track("/m/b.flac");
    covers.follow(Some(&a));
    let [request_a] = covers.take_requests().try_into().unwrap();
    assert_eq!(request_a.key, CoverKey::Local("/m/a.flac".into()));
    // The same track again asks nothing more.
    covers.follow(Some(&a));
    assert!(covers.take_requests().is_empty());
    // B is heard before A's cover arrives: A's result is not shown...
    covers.follow(Some(&b));
    let [request_b] = covers.take_requests().try_into().unwrap();
    assert!(!covers.handle(answer(&request_a, art())));
    assert!(covers.image().is_none());
    // ...but remembered: back on A, it shows at once, without a request.
    covers.follow(Some(&a));
    assert!(covers.take_requests().is_empty());
    assert!(covers.image().is_some());
    // B's late answer is remembered too, not shown over A.
    assert!(!covers.handle(answer(&request_b, Outcome::NoArt)));
    assert!(covers.image().is_some());
    covers.follow(Some(&b));
    assert!(covers.take_requests().is_empty(), "known: no art");
    assert!(covers.image().is_none());
    // Nothing heard: nothing shown.
    covers.follow(None);
    assert!(covers.image().is_none());
    assert!(!covers.reserve());
}

fn small_art(shade: u8) -> Arc<CoverImage> {
    Arc::new(solid(COVER_SIZE, COVER_SIZE, [shade, 1, 2]))
}

fn large_art(shade: u8) -> Arc<CoverImage> {
    Arc::new(solid(LARGE_COVER_SIZE, LARGE_COVER_SIZE, [shade, 1, 2]))
}

/// Covers with the large variant wanted, showing `track`'s small cover.
fn screen_showing(track: &Track, shade: u8) -> Covers {
    let mut covers = Covers::new(true);
    covers.set_large(true);
    covers.follow(Some(track));
    let [small] = covers.take_requests().try_into().unwrap();
    assert_eq!(small.size, CoverSize::Small, "the small one first");
    assert!(covers.handle(answer(&small, Outcome::Art(small_art(shade)))));
    covers
}

#[test]
fn the_large_cover_is_asked_for_after_the_small_one_and_replaces_it() {
    let a = remote_track("s1", Some("al-1"));
    let mut covers = screen_showing(&a, 1);
    // Meanwhile the small one shows, enlarged by the painter.
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
    let [large] = covers.take_requests().try_into().unwrap();
    assert_eq!(large.size, CoverSize::Large);
    assert_eq!(large.key, CoverKey::Navidrome("al-1".into()));
    // Asked once.
    assert!(covers.take_requests().is_empty());
    assert!(covers.handle(answer(&large, Outcome::Art(large_art(1)))));
    assert_eq!(covers.large_image().unwrap().width(), LARGE_COVER_SIZE);
    // The bar keeps the small one.
    assert_eq!(covers.image().unwrap().width(), COVER_SIZE);
    assert!(covers.take_requests().is_empty());
}

#[test]
fn large_covers_are_loaded_only_while_wanted() {
    let a = local_track("/m/a.flac");
    let mut covers = Covers::new(true);
    covers.follow(Some(&a));
    let [small] = covers.take_requests().try_into().unwrap();
    covers.handle(answer(&small, Outcome::Art(small_art(1))));
    assert!(covers.take_requests().is_empty(), "the screen is closed");
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
    covers.set_large(true);
    let [large] = covers.take_requests().try_into().unwrap();
    // Closed before it answered: forgotten, its answer ignored.
    covers.set_large(false);
    assert!(!covers.handle(answer(&large, Outcome::Art(large_art(1)))));
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
    // Opened again: asked again.
    covers.set_large(true);
    assert_eq!(covers.take_requests().len(), 1);
}

#[test]
fn no_large_cover_is_asked_for_a_small_source_or_while_the_small_loads() {
    let a = local_track("/m/a.flac");
    let mut covers = Covers::new(true);
    covers.set_large(true);
    covers.follow(Some(&a));
    let [small] = covers.take_requests().try_into().unwrap();
    assert!(covers.take_requests().is_empty(), "the small one loads");
    // A 2 px source: the small cover is all there is.
    covers.handle(answer(&small, art()));
    assert!(covers.take_requests().is_empty());
    assert_eq!(covers.large_image().unwrap().width(), 2);
}

#[test]
fn a_large_cover_that_fails_falls_back_to_the_small_one_until_the_track_changes() {
    let a = local_track("/m/a.flac");
    let b = local_track("/m/b.flac");
    let mut covers = screen_showing(&a, 1);
    let [large] = covers.take_requests().try_into().unwrap();
    assert!(!covers.handle(answer(&large, Outcome::Failed)));
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
    assert!(covers.take_requests().is_empty(), "not asked again at once");
    // Another track, then back: asked again.
    covers.follow(Some(&b));
    let [small_b] = covers.take_requests().try_into().unwrap();
    covers.handle(answer(&small_b, Outcome::NoArt));
    covers.follow(Some(&a));
    let [again] = covers.take_requests().try_into().unwrap();
    assert_eq!(again.size, CoverSize::Large, "the small one is remembered");
    // No art at the large size: the small one stays.
    assert!(!covers.handle(answer(&again, Outcome::NoArt)));
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
}

#[test]
fn tracks_with_the_same_picture_share_the_large_cover() {
    // Two tracks of an album: different keys, the same small pixels.
    let a = local_track("/m/album/1.flac");
    let b = local_track("/m/album/2.flac");
    let mut covers = screen_showing(&a, 1);
    let [large] = covers.take_requests().try_into().unwrap();
    covers.handle(answer(&large, Outcome::Art(large_art(1))));
    covers.follow(Some(&b));
    let [small_b] = covers.take_requests().try_into().unwrap();
    // While B's loads, A's large cover lingers (never a blank).
    assert_eq!(covers.large_image().unwrap().width(), LARGE_COVER_SIZE);
    covers.handle(answer(&small_b, Outcome::Art(small_art(1))));
    assert_eq!(covers.large_image().unwrap().width(), LARGE_COVER_SIZE);
    assert!(covers.take_requests().is_empty(), "nothing more to load");
}

#[test]
fn only_a_couple_of_large_covers_are_kept() {
    let mut covers = Covers::new(true);
    covers.set_large(true);
    let tracks: Vec<Track> = (0..3)
        .map(|i| local_track(&format!("/m/{i}.flac")))
        .collect();
    for (i, track) in tracks.iter().enumerate() {
        covers.follow(Some(track));
        let [small] = covers.take_requests().try_into().unwrap();
        covers.handle(answer(&small, Outcome::Art(small_art(i as u8))));
        let [large] = covers.take_requests().try_into().unwrap();
        covers.handle(answer(&large, Outcome::Art(large_art(i as u8))));
    }
    assert_eq!(LARGE_MEMORY_ENTRIES, 2);
    let large_shown = |covers: &mut Covers, track: &Track| {
        covers.follow(Some(track));
        let asked = covers.take_requests();
        (covers.large_image().unwrap().width(), asked.len())
    };
    assert_eq!(large_shown(&mut covers, &tracks[1]), (LARGE_COVER_SIZE, 0));
    // The oldest was dropped: its small cover shows and the large is asked
    // for again.
    assert_eq!(large_shown(&mut covers, &tracks[0]), (COVER_SIZE, 1));
}

#[test]
fn a_track_change_drops_the_large_load_running() {
    let a = local_track("/m/a.flac");
    let b = local_track("/m/b.flac");
    let mut covers = screen_showing(&a, 1);
    let [large] = covers.take_requests().try_into().unwrap();
    covers.follow(Some(&b));
    let [small_b] = covers.take_requests().try_into().unwrap();
    assert_eq!(small_b.size, CoverSize::Small);
    // A's large cover arriving late is not kept for B.
    assert!(!covers.handle(answer(&large, Outcome::Art(large_art(1)))));
    covers.handle(answer(&small_b, Outcome::Art(small_art(2))));
    assert_eq!(covers.large_image().unwrap().width(), COVER_SIZE);
}

#[test]
fn large_navidrome_results_from_another_client_are_dropped() {
    let a = remote_track("s1", Some("al-1"));
    let mut covers = screen_showing(&a, 1);
    let [large] = covers.take_requests().try_into().unwrap();
    covers.client_changed(Generation(1));
    assert!(!covers.handle(answer(&large, Outcome::Art(large_art(1)))));
    assert!(covers.large_image().is_none_or(|i| i.width() == COVER_SIZE));
}

#[test]
fn failures_are_not_remembered_and_nothing_is_retried_meanwhile() {
    let mut covers = Covers::new(true);
    let a = local_track("/m/a.flac");
    covers.follow(Some(&a));
    let [request] = covers.take_requests().try_into().unwrap();
    assert!(covers.handle(answer(&request, Outcome::Failed)));
    covers.follow(Some(&a));
    assert!(covers.take_requests().is_empty(), "no retry while heard");
    covers.follow(None);
    covers.follow(Some(&a));
    assert_eq!(covers.take_requests().len(), 1, "asked again next time");
}

#[test]
fn room_is_kept_while_the_next_cover_loads_after_one_with_art() {
    let mut covers = Covers::new(true);
    let (a, b, c) = (
        local_track("/m/a.flac"),
        local_track("/m/b.flac"),
        local_track("/m/c.flac"),
    );
    covers.follow(Some(&a));
    assert!(!covers.reserve(), "nothing shown yet");
    let [request] = covers.take_requests().try_into().unwrap();
    covers.handle(answer(&request, art()));
    assert!(covers.reserve());
    covers.follow(Some(&b));
    assert!(covers.reserve(), "loading after art");
    let [request] = covers.take_requests().try_into().unwrap();
    // B has none: A's, kept while B loaded, goes at once.
    assert!(covers.handle(answer(&request, Outcome::NoArt)));
    assert!(covers.image().is_none());
    assert!(!covers.reserve());
    covers.follow(Some(&c));
    assert!(!covers.reserve(), "loading after no art");
}

#[test]
fn navidrome_results_from_another_client_are_dropped() {
    let mut covers = Covers::new(true);
    let track = remote_track("s1", Some("al-1"));
    covers.follow(Some(&track));
    let [old] = covers.take_requests().try_into().unwrap();
    assert_eq!(old.key, CoverKey::Navidrome("al-1".into()));
    assert_eq!(old.generation, Generation(0));
    covers.client_changed(Generation(1));
    // The old client's answer is neither shown nor remembered.
    assert!(!covers.handle(answer(&old, art())));
    covers.follow(Some(&track));
    let [new] = covers.take_requests().try_into().unwrap();
    assert_eq!(new.generation, Generation(1));
    assert!(covers.image().is_none());
    assert!(covers.handle(answer(&new, art())));
    assert!(covers.image().is_some());
    // A swap forgets remembered Navidrome covers and asks again; the
    // cover stays on screen meanwhile, at most `LINGER`.
    covers.client_changed(Generation(2));
    covers.follow(Some(&track));
    assert_eq!(covers.take_requests().len(), 1);
    assert!(covers.image().is_some());
    assert!(covers.expire(Instant::now() + LINGER));
    assert!(covers.image().is_none());
}

#[test]
fn the_previous_cover_stays_until_the_next_result_or_a_second() {
    let mut covers = Covers::new(true);
    let (a, b, c) = (
        local_track("/m/a.flac"),
        local_track("/m/b.flac"),
        local_track("/m/c.flac"),
    );
    covers.follow(Some(&a));
    let [request] = covers.take_requests().try_into().unwrap();
    let shown_a = Arc::new(solid(2, 2, [1, 1, 1]));
    covers.handle(answer(&request, Outcome::Art(shown_a.clone())));
    // B loads: A stays.
    covers.follow(Some(&b));
    let [request_b] = covers.take_requests().try_into().unwrap();
    assert!(Arc::ptr_eq(covers.image().unwrap(), &shown_a));
    assert!(!covers.expire(Instant::now()), "not yet");
    // A stale answer (A's own, or one for another track) changes nothing.
    covers.follow(Some(&c));
    let [request_c] = covers.take_requests().try_into().unwrap();
    assert!(!covers.handle(answer(
        &request_b,
        Outcome::Art(Arc::new(solid(2, 2, [2, 2, 2])))
    )));
    assert!(Arc::ptr_eq(covers.image().unwrap(), &shown_a));
    // C's answer replaces it.
    let shown_c = Arc::new(solid(2, 2, [3, 3, 3]));
    assert!(covers.handle(answer(&request_c, Outcome::Art(shown_c.clone()))));
    assert!(Arc::ptr_eq(covers.image().unwrap(), &shown_c));
    // No answer within a second: removed.
    covers.follow(Some(&a)); // from memory: at once, no lingering
    assert!(covers.take_requests().is_empty());
    assert!(Arc::ptr_eq(covers.image().unwrap(), &shown_a));
    let d = local_track("/m/d.flac");
    covers.follow(Some(&d));
    assert!(covers.image().is_some());
    assert!(covers.expire(Instant::now() + LINGER));
    assert!(covers.image().is_none());
    // Nothing playing, or a track without a cover id: removed at once.
    covers.follow(Some(&a));
    covers.follow(None);
    assert!(covers.image().is_none());
    covers.follow(Some(&a));
    covers.follow(Some(&remote_track("s1", None)));
    assert!(covers.image().is_none());
}

#[test]
fn a_client_swap_while_a_navidrome_cover_loads_asks_the_new_client() {
    let mut covers = Covers::new(true);
    let track = remote_track("s1", Some("al-1"));
    // Shown first, so the bar has its art row.
    covers.follow(Some(&track));
    let [first] = covers.take_requests().try_into().unwrap();
    covers.handle(answer(&first, art()));
    covers.client_changed(Generation(1));
    covers.follow(Some(&track));
    let [old] = covers.take_requests().try_into().unwrap();
    assert!(covers.reserve(), "the bar keeps its height across the swap");
    covers.client_changed(Generation(2));
    covers.follow(Some(&track));
    assert!(
        covers.reserve(),
        "also while the swapped-out load was running"
    );
    let [new] = covers.take_requests().try_into().unwrap();
    assert_eq!(new.generation, Generation(2));
    // The old load, aborted by the new one, answers `Failed`: ignored.
    assert!(!covers.handle(answer(&old, Outcome::Failed)));
    assert!(covers.handle(answer(&new, art())));
    assert!(covers.image().is_some());
}

#[test]
fn local_covers_survive_a_client_swap() {
    let mut covers = Covers::new(true);
    let track = local_track("/m/a.flac");
    covers.follow(Some(&track));
    let [request] = covers.take_requests().try_into().unwrap();
    covers.client_changed(Generation(1));
    assert!(covers.handle(answer(&request, art())), "a file is a file");
    assert!(covers.image().is_some());
}

#[test]
fn remote_tracks_without_a_cover_id_ask_nothing() {
    let mut covers = Covers::new(true);
    covers.follow(Some(&remote_track("s1", None)));
    assert!(covers.take_requests().is_empty());
    assert!(covers.image().is_none());
}

#[test]
fn turned_off_nothing_is_requested() {
    let mut covers = Covers::new(false);
    covers.follow(Some(&local_track("/m/a.flac")));
    covers.follow(Some(&remote_track("s1", Some("al-1"))));
    assert!(covers.take_requests().is_empty());
    assert!(!covers.reserve());
}

#[test]
fn memory_keeps_the_most_recent_covers() {
    let mut covers = Covers::new(true);
    let tracks: Vec<Track> = (0..=MEMORY_ENTRIES)
        .map(|i| local_track(&format!("/m/{i}.flac")))
        .collect();
    for track in &tracks {
        covers.follow(Some(track));
        let [request] = covers.take_requests().try_into().unwrap();
        covers.handle(answer(&request, art()));
    }
    // The newest are remembered; the first was evicted.
    covers.follow(Some(&tracks[MEMORY_ENTRIES - 1]));
    assert!(covers.take_requests().is_empty());
    covers.follow(Some(&tracks[0]));
    assert_eq!(covers.take_requests().len(), 1);
}

#[test]
fn a_key_recognises_its_own_track_only() {
    let tracks = [
        local_track("/m/a.flac"),
        local_track("/m/b.flac"),
        remote_track("s1", Some("al-1")),
        remote_track("s2", Some("al-1")),
        remote_track("s3", Some("al-2")),
    ];
    for (i, track) in tracks.iter().enumerate() {
        let key = CoverKey::for_track(track).unwrap();
        for (j, other) in tracks.iter().enumerate() {
            let expected = CoverKey::for_track(other).as_ref() == Some(&key);
            assert_eq!(key.is_for(other), expected, "{i} {j}");
        }
    }
    // Two songs of one album share a cover: no new request between them.
    let mut covers = Covers::new(true);
    covers.follow(Some(&tracks[2]));
    assert_eq!(covers.take_requests().len(), 1);
    covers.follow(Some(&tracks[3]));
    assert!(covers.take_requests().is_empty());
}

#[test]
fn keys_never_show_paths_or_ids() {
    let local = format!("{:?}", CoverKey::Local("/home/me/secret.flac".into()));
    let remote = format!("{:?}", CoverKey::Navidrome("al-private".into()));
    assert!(!local.contains("secret") && !remote.contains("private"));
}

// ----- local covers ---------------------------------------------------

fn write_wav(path: &Path) {
    let data = vec![0u8; 800];
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes()); // format chunk size
    bytes.extend(1u16.to_le_bytes()); // PCM
    bytes.extend(1u16.to_le_bytes()); // mono
    bytes.extend(8000u32.to_le_bytes()); // sample rate
    bytes.extend(16000u32.to_le_bytes()); // byte rate
    bytes.extend(2u16.to_le_bytes()); // block align
    bytes.extend(16u16.to_le_bytes()); // bits per sample
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(&data);
    fs::write(path, bytes).unwrap();
}

/// Embeds pictures (type, PNG of that size) in the WAV at `path`.
fn embed(path: &Path, pictures: &[(lofty::picture::PictureType, u32)]) {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture};
    use lofty::prelude::TagExt;
    use lofty::tag::{Tag, TagType};
    let mut tag = Tag::new(TagType::Id3v2);
    for (kind, size) in pictures {
        tag.push_picture(
            Picture::unchecked(png(*size, *size))
                .pic_type(*kind)
                .mime_type(MimeType::Png)
                .build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

#[test]
fn an_embedded_front_cover_is_preferred() {
    use lofty::picture::PictureType;
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    fs::write(dir.path().join("cover.png"), png(20, 20)).unwrap();
    // The folder image, while the file has no picture.
    assert_eq!(local::load(&song).unwrap().width(), 20);
    // A front cover beats another picture and the folder image.
    embed(
        &song,
        &[(PictureType::Artist, 30), (PictureType::CoverFront, 40)],
    );
    assert_eq!(local::load(&song).unwrap().width(), 40);
    // Without a front cover, the first picture.
    embed(&song, &[(PictureType::Artist, 30)]);
    assert_eq!(local::load(&song).unwrap().width(), 30);
}

#[test]
fn very_large_files_use_the_folder_image() {
    use lofty::picture::PictureType;
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    embed(&song, &[(PictureType::CoverFront, 40)]);
    fs::write(dir.path().join("cover.png"), png(20, 20)).unwrap();
    assert_eq!(local::load(&song).unwrap().width(), 40);
    // Grown (sparsely) past the cap: the embedded picture is not read.
    let file = fs::OpenOptions::new().write(true).open(&song).unwrap();
    file.set_len(local::MAX_EMBEDDED_SCAN_BYTES + 1).unwrap();
    assert_eq!(local::load(&song).unwrap().width(), 20);
}

#[test]
fn folder_images_are_matched_case_insensitively_best_name_first() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    assert!(local::load(&song).is_none(), "no art at all");
    for (name, size) in [
        ("ALBUM.JPEG", 11),
        ("Front.png", 12),
        ("notes.png", 13),
        ("cover.gif", 14),
        ("Folder.Jpg", 15),
    ] {
        fs::write(dir.path().join(name), png(size, size)).unwrap();
    }
    let names: Vec<String> = local::folder_candidates(dir.path())
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Folder.Jpg", "Front.png", "ALBUM.JPEG"]);
    // `Folder.Jpg` holds a PNG: decoded by its bytes, not its name.
    assert_eq!(local::load(&song).unwrap().width(), 15);
    // A candidate that does not decode is skipped for the next.
    fs::write(dir.path().join("Folder.Jpg"), b"junk").unwrap();
    assert_eq!(local::load(&song).unwrap().width(), 12);
}

#[test]
fn oversized_and_special_folder_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    let mut big = png(8, 8);
    big.resize(MAX_IMAGE_BYTES + 1, 0);
    fs::write(dir.path().join("cover.jpg"), big).unwrap();
    fs::create_dir(dir.path().join("folder.png")).unwrap();
    assert!(local::load(&song).is_none());
    fs::write(dir.path().join("front.png"), png(9, 9)).unwrap();
    assert_eq!(local::load(&song).unwrap().width(), 9);
}

// ----- the disk cache -------------------------------------------------

#[test]
fn the_cache_folder_is_private_and_made_only_when_needed() {
    let dir = tempfile::tempdir().unwrap();
    let covers = dir.path().join("cache").join("coda").join("covers");
    let cache = DiskCache::new(covers.clone());
    assert_eq!(cache.dir(), covers);
    // Reading creates nothing.
    assert!(cache.get("x.jpg").is_none());
    assert!(!dir.path().join("cache").exists());
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let name = DiskCache::name("https://h/", "u", "c");
    cache.put(&name, &solid(8, 8, [1, 2, 3])).unwrap();
    assert_eq!(mode(&covers), 0o700);
    // Files are private too.
    assert_eq!(mode(&covers.join(&name)), 0o600);
    // An existing folder with wider permissions is narrowed.
    fs::set_permissions(&covers, fs::Permissions::from_mode(0o755)).unwrap();
    DiskCache::new(covers.clone())
        .put(&name, &solid(8, 8, [1, 2, 3]))
        .unwrap();
    assert_eq!(mode(&covers), 0o700);
}

#[test]
fn the_cache_location_follows_xdg() {
    assert_eq!(
        cache::cache_dir(Some("/x/cache".into()), Some("/home/me".into())),
        Some("/x/cache/coda/covers".into())
    );
    assert_eq!(
        cache::cache_dir(Some("relative".into()), Some("/home/me".into())),
        Some("/home/me/.cache/coda/covers".into())
    );
    assert_eq!(cache::cache_dir(None, None), None);
}

#[test]
fn cache_names_hold_no_secret_and_tell_servers_and_users_apart() {
    let name = DiskCache::name("https://music.example.org/nd/", "alice", "al-123_abc");
    assert!(name.ends_with(".jpg"));
    assert_eq!(name.len(), 32 + 4);
    assert!(name[..32].chars().all(|c| c.is_ascii_hexdigit()));
    for secret in ["music", "example", "alice", "al-123", "nd"] {
        assert!(!name.contains(secret), "{name}");
    }
    let others = [
        DiskCache::name("https://other.example.org/nd/", "alice", "al-123_abc"),
        DiskCache::name("https://music.example.org/nd/", "bob", "al-123_abc"),
        DiskCache::name("https://music.example.org/nd/", "alice", "al-124_abc"),
        // Fields cannot run into each other.
        DiskCache::name("https://music.example.org/nd/alice", "", "al-123_abc"),
    ];
    for other in others {
        assert_ne!(other, name);
    }
    assert_eq!(
        name,
        DiskCache::name("https://music.example.org/nd/", "alice", "al-123_abc")
    );
}

#[test]
fn cached_covers_round_trip_and_broken_entries_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let cache = DiskCache::new(dir.path().join("covers"));
    let name = DiskCache::name("https://h/", "u", "c1");
    assert!(cache.get(&name).is_none());
    cache.put(&name, &solid(64, 64, [200, 10, 10])).unwrap();
    let image = cache.get(&name).unwrap();
    assert_eq!((image.width(), image.height()), (64, 64));
    let [r, g, b] = image.pixel(32, 32);
    assert!(r > 180 && g < 40 && b < 40, "{r} {g} {b}");
    // No temp file is left behind.
    assert_eq!(fs::read_dir(cache.dir()).unwrap().count(), 1);
    // A corrupt entry is deleted, so the cover is fetched (and saved) again.
    let path = cache.dir().join(&name);
    fs::write(&path, b"\xFF\xD8\xFFgarbage").unwrap();
    assert!(cache.get(&name).is_none());
    assert!(!path.exists());
    cache.put(&name, &solid(8, 8, [0, 0, 0])).unwrap();
    assert!(cache.get(&name).is_some());
    // So is an oversized one, and a non-file in its place.
    fs::write(&path, vec![0u8; cache::MAX_ENTRY_BYTES as usize + 1]).unwrap();
    assert!(cache.get(&name).is_none());
    assert!(!path.exists());
    std::os::unix::fs::symlink("/etc/hosts", &path).unwrap();
    assert!(cache.get(&name).is_none());
    assert!(fs::symlink_metadata(&path).is_err(), "the link is removed");
}

#[test]
fn the_cache_evicts_least_recently_used_at_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let one = {
        let probe = DiskCache::new(dir.path().join("probe"));
        probe.put("x.jpg", &solid(64, 64, [5, 5, 5])).unwrap();
        fs::metadata(probe.dir().join("x.jpg")).unwrap().len()
    };
    // Room for about four entries; trimmed down to about two.
    let cache = DiskCache::with_limits(dir.path().join("covers"), one * 4, one * 2);
    let names: Vec<String> = (0..4).map(|i| format!("{i}.jpg")).collect();
    let old = SystemTime::now() - Duration::from_secs(3600);
    for (i, name) in names.iter().enumerate() {
        cache.put(name, &solid(64, 64, [5, 5, 5])).unwrap();
        // Distinct ages, oldest first.
        let file = fs::OpenOptions::new()
            .write(true)
            .open(cache.dir().join(name))
            .unwrap();
        file.set_modified(old + Duration::from_secs(i as u64 * 60))
            .unwrap();
    }
    // Using the oldest makes it the newest.
    assert!(cache.get(&names[0]).is_some());
    cache.put("4.jpg", &solid(64, 64, [5, 5, 5])).unwrap();
    let left: Vec<bool> = names.iter().map(|n| cache.dir().join(n).exists()).collect();
    assert_eq!(left, [true, false, false, false]);
    assert!(cache.dir().join("4.jpg").exists());
}

#[test]
fn local_covers_load_at_the_large_size() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    fs::write(dir.path().join("cover.jpg"), jpeg(1200, 1200)).unwrap();
    assert_eq!(local::load(&song).unwrap().width(), COVER_SIZE);
    assert_eq!(
        local::load_at(&song, LARGE_COVER_SIZE).unwrap().width(),
        LARGE_COVER_SIZE
    );
}

#[test]
fn large_covers_have_their_own_cache_beside_the_small_one() {
    let dir = tempfile::tempdir().unwrap();
    let covers = dir.path().join("covers");
    let small = DiskCache::with_limits(covers.clone(), 1, 1);
    let large = DiskCache::large(cache::large_dir(&covers));
    assert_eq!(large.dir(), covers.join("large"));
    assert_eq!(
        (small.size(), large.size()),
        (CoverSize::Small, CoverSize::Large)
    );
    let name = DiskCache::name("https://h/", "u", "c");
    large
        .put(
            &name,
            &solid(LARGE_COVER_SIZE, LARGE_COVER_SIZE, [9, 99, 199]),
        )
        .unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(large.dir()), 0o700);
    // Read back at the large size.
    assert_eq!(large.get(&name).unwrap().width(), LARGE_COVER_SIZE);
    // The small cache, trimmed to nothing on every write, never touches
    // the large folder.
    small.put(&name, &solid(8, 8, [1, 2, 3])).unwrap();
    assert!(large.dir().join(&name).exists());
    assert_eq!(small.get(&name).map(|i| i.width()), None, "trimmed");
    // A large entry may be bigger than a small one, up to its own limit.
    let path = large.dir().join(&name);
    fs::write(&path, vec![0u8; cache::MAX_ENTRY_BYTES as usize + 1]).unwrap();
    assert!(large.get(&name).is_none(), "not an image: removed");
    let mut big = jpeg(64, 64);
    big.resize(cache::MAX_ENTRY_BYTES as usize + 1, 0);
    fs::write(&path, &big).unwrap();
    assert_eq!(large.get(&name).map(|i| i.width()), Some(64));
    big.resize(cache::MAX_LARGE_ENTRY_BYTES as usize + 1, 0);
    fs::write(&path, &big).unwrap();
    assert!(large.get(&name).is_none());
    assert!(!path.exists());
}

// ----- loading --------------------------------------------------------

#[tokio::test]
async fn a_local_load_answers_with_the_cover() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.wav");
    write_wav(&song);
    fs::write(dir.path().join("cover.png"), png(10, 10)).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = CoverRequest {
        id: CoverRequestId(3),
        generation: Generation(0),
        key: CoverKey::Local(song),
        size: CoverSize::Small,
    };
    spawn_load(request.clone(), None, None, &tx).await.unwrap();
    let response = rx.recv().await.unwrap();
    assert_eq!(response.id, CoverRequestId(3));
    assert!(matches!(response.outcome, Outcome::Art(image) if image.width() == 10));
    // Navidrome without a client: no art.
    let remote = CoverRequest {
        key: CoverKey::Navidrome("al-1".into()),
        ..request
    };
    spawn_load(remote, None, None, &tx).await.unwrap();
    assert!(matches!(rx.recv().await.unwrap().outcome, Outcome::NoArt));
}

#[tokio::test]
async fn an_aborted_load_still_answers() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let request = CoverRequest {
        id: CoverRequestId(9),
        generation: Generation(0),
        key: CoverKey::Local("/nonexistent/x.flac".into()),
        size: CoverSize::Small,
    };
    let task = spawn_load(request, None, None, &tx);
    // Aborted before its first poll.
    task.abort();
    let _ = task.await;
    let response = rx.recv().await.unwrap();
    assert_eq!(response.id, CoverRequestId(9));
    assert!(matches!(response.outcome, Outcome::Failed));
}

#[test]
fn encode_jpeg_round_trips_a_cover() {
    let image = decode(&png(40, 40)).unwrap();
    let jpeg = encode_jpeg(&image).unwrap();
    assert_eq!(sniff(&jpeg), Some(Format::Jpeg));
    let back = decode(&jpeg).unwrap();
    assert_eq!((back.width(), back.height()), (40, 40));
}

#[tokio::test]
async fn load_reads_a_local_folder_cover_at_the_size_asked() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("cover.png"), png(600, 600)).unwrap();
    let key = CoverKey::Local(dir.path().join("track.flac"));
    match load(&key, CoverSize::Small, None, None).await {
        Outcome::Art(image) => assert_eq!(image.width(), COVER_SIZE),
        other => panic!("{other:?}"),
    }
    let none = CoverKey::Local(dir.path().join("sub").join("track.flac"));
    assert!(matches!(
        load(&none, CoverSize::Large, None, None).await,
        Outcome::NoArt
    ));
    // A Navidrome cover without a client has no art.
    let remote = CoverKey::Navidrome("al-1".into());
    assert!(matches!(
        load(&remote, CoverSize::Small, None, None).await,
        Outcome::NoArt
    ));
}
