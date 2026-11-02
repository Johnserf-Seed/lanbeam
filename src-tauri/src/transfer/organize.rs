//! Auto-organize (M6.6): the subfolder a received file is written under.
//!
//! `device` and `date` put a whole transfer in one folder. `type` (F7) decides
//! file by file: a loose file goes into the folder for its kind, while a file
//! that came inside a sent folder keeps that folder whole at the root — the
//! folder is what the sender chose to send, and splitting its contents across
//! 图片/ and 文档/ would take it apart.

use std::path::{Path, PathBuf};

/// Where auto-organize puts a transfer's files.
pub(super) enum Organize {
    /// Straight under the download root.
    Flat,
    /// The whole transfer under one folder: the sender's name, or today's date.
    Folder(PathBuf),
    /// Each loose file under the folder for its kind, named in this language.
    ByType(FolderLang),
}

impl Organize {
    /// Where the file at `rel` (sanitized, relative to the download root) lands.
    pub(super) fn place(&self, rel: &Path) -> PathBuf {
        match self {
            Organize::Flat => rel.to_path_buf(),
            Organize::Folder(prefix) => prefix.join(rel),
            Organize::ByType(lang) => {
                if rel.components().count() > 1 {
                    return rel.to_path_buf();
                }
                Path::new(Kind::of(rel).folder(*lang)).join(rel)
            }
        }
    }
}

/// The language the by-type folders are named in: the UI's, as the frontend
/// last reported it (`Settings::ui_lang`). The backend has no i18n of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FolderLang {
    Zh,
    En,
}

impl FolderLang {
    /// `"zh"` → Chinese; anything else, including never reported, → English.
    pub(super) fn from_setting(ui_lang: &str) -> FolderLang {
        if ui_lang == "zh" {
            FolderLang::Zh
        } else {
            FolderLang::En
        }
    }
}

/// What a file is, going by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Installer,
    Other,
}

impl Kind {
    pub(super) fn of(name: &Path) -> Kind {
        let Some(ext) = name.extension().and_then(|e| e.to_str()) else {
            return Kind::Other;
        };
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "apng" | "gif" | "webp" | "heic" | "heif"
            | "avif" | "jxl" | "bmp" | "tif" | "tiff" | "svg" | "ico" | "icns" | "psd" | "raw"
            | "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "rw2" | "raf" | "srw" => Kind::Image,
            "mp4" | "m4v" | "mov" | "avi" | "mkv" | "webm" | "flv" | "f4v" | "wmv" | "asf"
            | "3gp" | "3g2" | "mpg" | "mpeg" | "m2ts" | "mts" | "vob" | "ogv" | "rm" | "rmvb" => {
                Kind::Video
            }
            "mp3" | "m4a" | "m4b" | "aac" | "wav" | "flac" | "ogg" | "oga" | "opus" | "wma"
            | "aif" | "aiff" | "aifc" | "ape" | "amr" | "mid" | "midi" | "caf" | "wv" => {
                Kind::Audio
            }
            // WPS (wps/et/dps) and OFD — the formats Chinese offices and
            // e-invoices come in — sit beside the Office and iWork ones.
            "pdf" | "doc" | "docx" | "docm" | "dot" | "dotx" | "xls" | "xlsx" | "xlsm" | "xlsb"
            | "csv" | "tsv" | "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "odt" | "ods" | "odp"
            | "odg" | "rtf" | "txt" | "md" | "markdown" | "pages" | "numbers" | "key" | "epub"
            | "mobi" | "azw" | "azw3" | "djvu" | "wps" | "wpt" | "et" | "ett" | "dps" | "dpt"
            | "ofd" | "xps" | "oxps" | "tex" => Kind::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2" | "xz" | "txz"
            | "zst" | "tzst" | "lz" | "lzma" | "lz4" | "z" | "cab" | "arj" | "iso" => Kind::Archive,
            "exe" | "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "dmg" | "pkg"
            | "mpkg" | "apk" | "apks" | "xapk" | "ipa" | "deb" | "rpm" | "appimage" | "snap" => {
                Kind::Installer
            }
            _ => Kind::Other,
        }
    }

    pub(super) fn folder(self, lang: FolderLang) -> &'static str {
        match (self, lang) {
            (Kind::Image, FolderLang::Zh) => "图片",
            (Kind::Image, FolderLang::En) => "Images",
            (Kind::Video, FolderLang::Zh) => "视频",
            (Kind::Video, FolderLang::En) => "Videos",
            (Kind::Audio, FolderLang::Zh) => "音频",
            (Kind::Audio, FolderLang::En) => "Audio",
            (Kind::Document, FolderLang::Zh) => "文档",
            (Kind::Document, FolderLang::En) => "Documents",
            (Kind::Archive, FolderLang::Zh) => "压缩包",
            (Kind::Archive, FolderLang::En) => "Archives",
            (Kind::Installer, FolderLang::Zh) => "安装包",
            (Kind::Installer, FolderLang::En) => "Installers",
            (Kind::Other, FolderLang::Zh) => "其他",
            (Kind::Other, FolderLang::En) => "Other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> Kind {
        Kind::of(Path::new(name))
    }

    #[test]
    fn kinds_follow_the_extension_in_any_case() {
        assert_eq!(kind("IMG_0001.HEIC"), Kind::Image);
        assert_eq!(kind("photo.jpg"), Kind::Image);
        assert_eq!(kind("clip.MOV"), Kind::Video);
        assert_eq!(kind("song.flac"), Kind::Audio);
        assert_eq!(kind("report.pdf"), Kind::Document);
        assert_eq!(kind("发票.ofd"), Kind::Document);
        assert_eq!(kind("backup.tar.gz"), Kind::Archive);
        assert_eq!(kind("LanBeam.dmg"), Kind::Installer);
        assert_eq!(kind("setup.EXE"), Kind::Installer);
    }

    #[test]
    fn unknown_or_missing_extensions_are_other() {
        assert_eq!(kind("Makefile"), Kind::Other);
        assert_eq!(kind(".bashrc"), Kind::Other);
        assert_eq!(kind("main.rs"), Kind::Other);
        assert_eq!(kind("trailing."), Kind::Other);
    }

    #[test]
    fn every_kind_has_a_folder_in_both_languages() {
        let kinds = [
            Kind::Image,
            Kind::Video,
            Kind::Audio,
            Kind::Document,
            Kind::Archive,
            Kind::Installer,
            Kind::Other,
        ];
        for lang in [FolderLang::Zh, FolderLang::En] {
            let mut names: Vec<_> = kinds.iter().map(|k| k.folder(lang)).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), kinds.len(), "two kinds share a folder");
        }
    }

    #[test]
    fn by_type_sorts_loose_files_and_keeps_sent_folders_whole() {
        let zh = Organize::ByType(FolderLang::Zh);
        assert_eq!(
            zh.place(Path::new("photo.jpg")),
            Path::new("图片").join("photo.jpg")
        );
        assert_eq!(
            zh.place(Path::new("notes")),
            Path::new("其他").join("notes")
        );
        let en = Organize::ByType(FolderLang::En);
        assert_eq!(
            en.place(Path::new("report.pdf")),
            Path::new("Documents").join("report.pdf")
        );
        let in_folder = Path::new("Trip").join("day1").join("photo.jpg");
        assert_eq!(zh.place(&in_folder), in_folder);
    }

    #[test]
    fn device_and_date_put_everything_under_one_folder() {
        let rel = Path::new("Trip").join("photo.jpg");
        let folder = Organize::Folder(PathBuf::from("2026-11-02"));
        assert_eq!(folder.place(&rel), Path::new("2026-11-02").join(&rel));
        assert_eq!(Organize::Flat.place(&rel), rel);
    }

    #[test]
    fn only_zh_names_folders_in_chinese() {
        assert_eq!(FolderLang::from_setting("zh"), FolderLang::Zh);
        assert_eq!(FolderLang::from_setting("en"), FolderLang::En);
        assert_eq!(FolderLang::from_setting(""), FolderLang::En);
    }
}
