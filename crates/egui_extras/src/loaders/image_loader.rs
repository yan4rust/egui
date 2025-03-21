use ahash::HashMap;
use egui::{
    decode_animated_image_uri,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
    ColorImage,
};
use image::ImageFormat;
use std::{mem::size_of, path::Path, sync::Arc};

type Entry = Result<Arc<ColorImage>, LoadError>;

#[derive(Default)]
pub struct ImageCrateLoader {
    cache: Mutex<HashMap<String, Entry>>,
}

impl ImageCrateLoader {
    pub const ID: &'static str = egui::generate_loader_id!(ImageCrateLoader);
}

fn is_supported_uri(uri: &str) -> bool {
    let Some(ext) = Path::new(uri)
        .extension()
        .and_then(|ext| ext.to_str().map(|ext| ext.to_lowercase()))
    else {
        // `true` because if there's no extension, assume that we support it
        return true;
    };

    // Uses only the enabled image crate features
    ImageFormat::from_extension(ext).is_some_and(|format| format.reading_enabled())
}

fn is_supported_mime(mime: &str) -> bool {
    // some mime types e.g. reflect binary files or mark the content as a download, which
    // may be a valid image or not, in this case, defer the decision on the format guessing
    // or the image crate and return true here
    let mimes_to_defer = [
        "application/octet-stream",
        "application/x-msdownload",
        "application/force-download",
    ];
    for m in &mimes_to_defer {
        // use contains instead of direct equality, as e.g. encoding info might be appended
        if mime.contains(m) {
            return true;
        }
    }

    // Uses only the enabled image crate features
    ImageFormat::from_mime_type(mime).is_some_and(|format| format.reading_enabled())
}

impl ImageLoader for ImageCrateLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        // three stages of guessing if we support loading the image:
        // 1. URI extension (only done for files)
        // 2. Mime from `BytesPoll::Ready`
        // 3. image::guess_format (used internally by image::load_from_memory)

        // TODO(lucasmerlin): Egui currently changes all URIs for webp and gif files to include
        // the frame index (#0), which breaks if the animated image loader is disabled.
        // We work around this by removing the frame index from the URI here
        let uri = decode_animated_image_uri(uri).map_or(uri, |(uri, _frame_index)| uri);

        // (1)
        if uri.starts_with("file://") && !is_supported_uri(uri) {
            return Err(LoadError::NotSupported);
        }

        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(uri).cloned() {
            match entry {
                Ok(image) => Ok(ImagePoll::Ready { image }),
                Err(err) => Err(err),
            }
        } else {
            match ctx.try_load_bytes(uri) {
                Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                    // (2)
                    if let Some(mime) = mime {
                        if !is_supported_mime(&mime) {
                            return Err(LoadError::FormatNotSupported {
                                detected_format: Some(mime),
                            });
                        }
                    }

                    if bytes.starts_with(b"version https://git-lfs") {
                        return Err(LoadError::FormatNotSupported {
                            detected_format: Some("git-lfs".to_owned()),
                        });
                    }

                    // (3)
                    log::trace!("started loading {uri:?}");
                    let result = crate::image::load_image_bytes(&bytes).map(Arc::new);
                    log::trace!("finished loading {uri:?}");
                    cache.insert(uri.into(), result.clone());
                    result.map(|image| ImagePoll::Ready { image })
                }
                Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
                Err(err) => Err(err),
            }
        }
    }

    fn forget(&self, uri: &str) {
        let _ = self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|result| match result {
                Ok(image) => image.pixels.len() * size_of::<egui::Color32>(),
                Err(err) => err.byte_size(),
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_support() {
        assert!(is_supported_uri("https://test.png"));
        assert!(is_supported_uri("test.jpeg"));
        assert!(is_supported_uri("http://test.gif"));
        assert!(is_supported_uri("file://test"));
        assert!(!is_supported_uri("test.svg"));
    }
}
