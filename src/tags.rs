use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

/// The fields the tag editor exposes. Everything is kept as text so the
/// widget can bind directly to it; numbers are parsed when saving.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagData {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track: String,
    pub genre: String,
    pub lyrics: String,
}

pub fn read(path: &Path) -> Result<TagData, String> {
    let tagged = lofty::probe::Probe::open(path)
        .and_then(|probe| probe.read())
        .map_err(|error| format!("failed to read tags: {error}"))?;

    let mut data = TagData::default();
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(data);
    };

    data.title = text(tag.title());
    data.artist = text(tag.artist());
    data.album = text(tag.album());
    data.album_artist = text(tag.get_string(ItemKey::AlbumArtist));
    data.year = year_text(
        tag.get_string(ItemKey::Year)
            .or_else(|| tag.get_string(ItemKey::RecordingDate)),
    );
    data.track = tag
        .track()
        .map(|number| number.to_string())
        .unwrap_or_default();
    data.genre = text(tag.genre());
    data.lyrics = text(
        tag.get_string(ItemKey::UnsyncLyrics)
            .or_else(|| tag.get_string(ItemKey::Lyrics)),
    );
    Ok(data)
}

pub fn save(path: &Path, data: &TagData) -> Result<(), String> {
    let year = parse_year(&data.year)?;
    let track = parse_track(&data.track)?;

    let mut tagged = lofty::probe::Probe::open(path)
        .and_then(|probe| probe.read())
        .map_err(|error| format!("failed to read file: {error}"))?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .primary_tag_mut()
        .ok_or_else(|| "this format does not support tags".to_string())?;

    set(tag, ItemKey::TrackTitle, &data.title);
    set(tag, ItemKey::TrackArtist, &data.artist);
    set(tag, ItemKey::AlbumTitle, &data.album);
    set(tag, ItemKey::AlbumArtist, &data.album_artist);
    set(
        tag,
        ItemKey::RecordingDate,
        &year.map(|year| year.to_string()).unwrap_or_default(),
    );
    tag.remove_key(ItemKey::Year);
    set(tag, ItemKey::Genre, &data.genre);

    match track {
        Some(number) => tag.set_track(number),
        None => tag.remove_track(),
    }

    // ID3v2 only knows USLT for plain lyrics. Other containers use LYRICS.
    let (lyrics_key, stale_key) = if tag_type == TagType::Id3v2 {
        (ItemKey::UnsyncLyrics, ItemKey::Lyrics)
    } else {
        (ItemKey::Lyrics, ItemKey::UnsyncLyrics)
    };
    set(tag, lyrics_key, &data.lyrics);
    tag.remove_key(stale_key);

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|error| format!("failed to write tags: {error}"))
}

fn text<T: AsRef<str>>(value: Option<T>) -> String {
    value
        .map(|value| value.as_ref().to_string())
        .unwrap_or_default()
}

fn year_text<T: AsRef<str>>(value: Option<T>) -> String {
    let value = text(value);
    let head = value.get(..4).unwrap_or(&value);
    if head.len() == 4 && head.chars().all(|ch| ch.is_ascii_digit()) {
        head.to_string()
    } else {
        value
    }
}

fn set(tag: &mut Tag, key: ItemKey, value: &str) {
    if value.trim().is_empty() {
        tag.remove_key(key);
    } else {
        tag.insert_text(key, value.to_string());
    }
}

fn parse_year(value: &str) -> Result<Option<i32>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let year = value
        .parse::<i32>()
        .map_err(|_| "year: an integer is required".to_string())?;
    if !(1..=9999).contains(&year) {
        return Err("year: a number from 1 to 9999 is required".to_string());
    }
    Ok(Some(year))
}

fn parse_track(value: &str) -> Result<Option<u32>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let track = value
        .parse::<u32>()
        .map_err(|_| "track number: an integer is required".to_string())?;
    if track == 0 {
        return Err("track number: a number greater than zero is required".to_string());
    }
    Ok(Some(track))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_validated() {
        assert_eq!(parse_year("").unwrap(), None);
        assert_eq!(parse_year("1997").unwrap(), Some(1997));
        assert!(parse_year("19x7").is_err());
        assert!(parse_year("0").is_err());

        assert_eq!(parse_track("").unwrap(), None);
        assert_eq!(parse_track("7").unwrap(), Some(7));
        assert!(parse_track("-1").is_err());
        assert!(parse_track("0").is_err());
    }
}
