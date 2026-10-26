/// The one extension-to-family table (#908). Case-insensitive; every answer is a member of
/// `FAMILIES`. No extension is `none`; an extension nobody classified is `other`, never a guess.
/// Ambiguous extensions (`bin`, `dat`, `img`, `log`, `h5`, `pb`, `com`) are deliberately absent.
pub fn extension_family(ext: Option<&str>) -> &'static str {
    let Some(ext) = ext else { return "none" };
    let mut buf = [0u8; 16];
    let Some(dst) = buf.get_mut(..ext.len()) else {
        return "other";
    };
    dst.copy_from_slice(ext.as_bytes());
    dst.make_ascii_lowercase();
    match std::str::from_utf8(dst).unwrap_or("") {
        "doc" | "docx" | "docm" | "dot" | "dotx" | "odt" | "rtf" | "txt" | "md" | "markdown"
        | "pdf" | "epub" | "mobi" | "azw3" | "pages" | "tex" | "wpd" | "xps" | "djvu" => "document",
        "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "ods" | "numbers" => "spreadsheet",
        "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" | "odp" | "key" => {
            "presentation"
        }
        "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp"
        | "svg" | "ico" | "heic" | "heif" | "avif" | "psd" | "ai" | "xcf" | "tga" | "exr"
        // Camera RAW.
        | "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" | "pef" | "srw" => "image",
        "mp4" | "m4v" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "mpg" | "mpeg" | "m2ts"
        | "mts" | "3gp" | "vob" | "ogv" => "video",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "oga" | "opus" | "wma" | "aiff" | "aif"
        | "mid" | "midi" | "amr" => "audio",
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "lz" | "lzma" | "z"
        | "cab" | "iso" => "archive",
        "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "cc"
        | "cxx" | "hpp" | "cs" | "java" | "kt" | "go" | "rb" | "php" | "swift" | "sh" | "bash"
        | "ps1" | "psm1" | "bat" | "cmd" | "lua" | "pl" | "r" | "sql" | "html" | "htm" | "css"
        | "scss" | "sass" | "less" | "vue" | "svelte" | "dart" | "scala" | "hs" | "ex" | "exs"
        | "clj" | "asm" => "code",
        "json" | "jsonl" | "ndjson" | "xml" | "yaml" | "yml" | "toml" | "csv" | "tsv" | "ini"
        | "cfg" | "conf" | "parquet" | "arrow" | "feather" | "avro" | "orc" | "db" | "sqlite"
        | "sqlite3" | "mdb" | "accdb" | "npy" | "npz" | "pkl" | "pickle" => "data",
        "safetensors" | "gguf" | "ggml" | "onnx" | "ckpt" | "pt" | "pth" | "tflite" => "model",
        "exe" | "msi" | "msix" | "appx" | "apk" | "jar" | "scr" => "executable",
        "dll" | "sys" | "drv" | "ocx" | "cpl" | "lnk" | "inf" | "cat" | "mui" | "evtx" | "etl"
        | "reg" | "dmp" | "pf" => "system",
        "ttf" | "otf" | "woff" | "woff2" | "ttc" | "eot" | "fon" | "pfb" => "font",
        _ => "other",
    }
}
