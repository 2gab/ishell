//! SPDX-License-Identifier: MIT

#[cfg(any(
    feature = "cover-viuer-iterm",
    feature = "cover-viuer-kitty",
    feature = "cover-viuer-sixel"
))]
use std::io::Write;

#[cfg(any(
    feature = "cover-viuer-iterm",
    feature = "cover-viuer-kitty",
    feature = "cover-viuer-sixel"
))]
use anyhow::Context;
use std::sync::LazyLock;

use anyhow::Result;
use image::DynamicImage;
use termusiclib::track::MediaTypes;
use termusiclib::xywh::Xywh;
use tokio::runtime::Handle;

use crate::ui::ids::{Id, IdConfigEditor, IdTagEditor};
use crate::ui::model::{Model, Panel, TxToMain, VISUALIZER_HEIGHT, ViuerSupported};
use crate::ui::msg::{CoverDLResult, ImageWrapper, Msg, XYWHMsg};

/// Bundled placeholder cover, shown for tracks with no embedded or folder cover art.
static DEFAULT_COVER_BYTES: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/default_cover.png"));

/// Decoded [`DEFAULT_COVER_BYTES`], decoded once and reused.
static DEFAULT_COVER: LazyLock<DynamicImage> = LazyLock::new(|| {
    image::load_from_memory(DEFAULT_COVER_BYTES)
        .expect("Failed to decode bundled default cover image")
});

impl Model {
    pub fn xywh_move_left(&mut self) {
        self.xywh.move_left();
        self.update_photo().ok();
    }

    pub fn xywh_move_right(&mut self) {
        self.xywh.move_right();
        self.update_photo().ok();
    }

    pub fn xywh_move_up(&mut self) {
        self.xywh.move_up();
        self.update_photo().ok();
    }

    pub fn xywh_move_down(&mut self) {
        self.xywh.move_down();
        self.update_photo().ok();
    }
    pub fn xywh_zoom_in(&mut self) {
        self.xywh.zoom_in();
        self.update_photo().ok();
    }
    pub fn xywh_zoom_out(&mut self) {
        self.xywh.zoom_out();
        self.update_photo().ok();
    }
    pub fn xywh_toggle_hide(&mut self) {
        self.clear_photo().ok();
        let mut config_tui = self.config_tui.write();

        // dont save value if cli has overwritten it, but still allow runtime changing
        if let Some(current) = config_tui.coverart_hidden_overwrite {
            config_tui.coverart_hidden_overwrite = Some(!current);
            info!("Not saving coverart.hidden as it is overwritten by cli!");
        } else {
            config_tui.settings.coverart.hidden = !config_tui.settings.coverart.hidden;
        }

        drop(config_tui);
        self.update_photo().ok();
    }
    fn should_not_show_photo(&self) -> bool {
        if self.app.mounted(&Id::HelpPopup) {
            return true;
        }
        if self.app.mounted(&Id::PodcastSearchTablePopup) {
            return true;
        }

        if self.app.mounted(&Id::TagEditor(IdTagEditor::InputTitle)) {
            return true;
        }

        if self.app.mounted(&Id::GeneralSearchInput) {
            return true;
        }

        if self.playback.is_stopped() {
            return true;
        }

        if self.app.mounted(&Id::ConfigEditor(IdConfigEditor::Header)) {
            return true;
        }

        false
    }

    /// Get and show a image for the current playing media
    ///
    /// Requires that the current thread has a entered runtime
    #[allow(clippy::cast_possible_truncation)]
    pub fn update_photo(&mut self) -> Result<()> {
        if self.config_tui.read().get_coverart_hidden() {
            return Ok(());
        }
        self.clear_photo()?;

        if self.should_not_show_photo() {
            return Ok(());
        }
        let Some(track) = self.playback.current_track() else {
            return Ok(());
        };

        match track.inner() {
            MediaTypes::Track(track_data) => {
                let picture = match track.get_picture() {
                    Ok(v) => v,
                    Err(err) => {
                        error!(
                            "Getting the cover for \"{}\" failed! Error: {}",
                            track_data.path().display(),
                            err
                        );
                        None
                    }
                };

                let image = picture.and_then(|picture| image::load_from_memory(picture.data()).ok());

                match image {
                    Some(image) => self.show_image(&image)?,
                    None => self.show_image(&DEFAULT_COVER)?,
                }
                return Ok(());
            }
            MediaTypes::Radio(_radio_track_data) => (),
            MediaTypes::Podcast(podcast_track_data) => {
                let url = {
                    if let Some(episode_photo_url) = podcast_track_data.image_url() {
                        episode_photo_url.to_string()
                    } else if let Some(pod_photo_url) =
                        self.podcast_get_album_photo_by_url(podcast_track_data.url())
                    {
                        pod_photo_url
                    } else {
                        return Ok(());
                    }
                };

                if url.is_empty() {
                    return Ok(());
                }
                let tx = self.tx_to_main.clone();

                Handle::current().spawn(Self::fetch_podcast_image(tx, url));
            }
        }

        Ok(())
    }

    /// Fetch the given url as a image and send events when done or error.
    async fn fetch_podcast_image(tx: TxToMain, url: String) {
        match reqwest::get(&url).await {
            Ok(result) => {
                if result.status() != reqwest::StatusCode::OK {
                    tx.send(Msg::Xywh(XYWHMsg::CoverDLResult(
                        CoverDLResult::FetchPhotoErr(format!(
                            "Error non-OK Status code: {}",
                            result.status()
                        )),
                    )))
                    .ok();
                    return;
                }

                let cursor = {
                    let bytes = match result.bytes().await {
                        Ok(v) => v,
                        Err(err) => {
                            tx.send(Msg::Xywh(XYWHMsg::CoverDLResult(
                                CoverDLResult::FetchPhotoErr(format!(
                                    "Error in reqest::Response::bytes: {err}"
                                )),
                            )))
                            .ok();
                            return;
                        }
                    };

                    std::io::Cursor::new(bytes)
                };

                let image = match image::ImageReader::new(cursor).with_guessed_format() {
                    Ok(v) => v,
                    Err(err) => {
                        let _ = tx.send(Msg::Xywh(XYWHMsg::CoverDLResult(
                            CoverDLResult::FetchPhotoErr(format!(
                                "Failed to get a valid format for downloaded image: {err}"
                            )),
                        )));
                        return;
                    }
                };

                match image.decode() {
                    Ok(image) => {
                        let image_wrapper = ImageWrapper { data: image };
                        tx.send(Msg::Xywh(XYWHMsg::CoverDLResult(
                            CoverDLResult::FetchPhotoSuccess(image_wrapper),
                        )))
                        .ok()
                    }
                    Err(e) => tx
                        .send(Msg::Xywh(XYWHMsg::CoverDLResult(
                            CoverDLResult::FetchPhotoErr(format!(
                                "Decoding downloaded image failed: {e}"
                            )),
                        )))
                        .ok(),
                }
            }
            Err(e) => tx
                .send(Msg::Xywh(XYWHMsg::CoverDLResult(
                    CoverDLResult::FetchPhotoErr(format!("Error in ureq get: {e}")),
                )))
                .ok(),
        };
    }

    /// In bare `:player` mode (`NowPlaying` + `Progress`, nothing else), compute a cover box that
    /// exactly fits the blank [`Panel::Spacer`] gap between them, centered — so the (bigger)
    /// image never overlaps their borders/text. Returns `None` for every other panel
    /// combination, in which case the normal config-driven [`Xywh`] positioning applies unchanged.
    ///
    /// Deliberately duplicates (rather than calls into) the layout math in `ui::model::view`,
    /// which is private to that module — small cross-module duplication over widening visibility,
    /// per this codebase's convention.
    #[allow(clippy::cast_possible_truncation)]
    fn player_cover_xywh(&self, image: &DynamicImage) -> Option<Xywh> {
        if !self.visible_panels.contains(&Panel::Spacer) {
            return None;
        }

        let (term_width, term_height) = Xywh::get_terminal_size_u32();

        const FOOTER_HEIGHT: u32 = 1;
        const NOW_PLAYING_HEIGHT: u32 = 3;
        const PROGRESS_HEIGHT: u32 = 3;
        const MARGIN_X: u32 = 2;
        const MARGIN_Y: u32 = 1;

        let visualizer_height = if self.show_visualizer {
            u32::from(VISUALIZER_HEIGHT)
        } else {
            0
        };
        let chunks_main_height = term_height.saturating_sub(visualizer_height + FOOTER_HEIGHT);

        let (panel_x, panel_width) = if self.show_sidebar {
            let left_width = term_width / 3;
            (left_width, term_width - left_width)
        } else {
            (0, term_width)
        };

        let spacer_height =
            chunks_main_height.checked_sub(NOW_PLAYING_HEIGHT + PROGRESS_HEIGHT)?;
        let spacer_y = NOW_PLAYING_HEIGHT;

        let avail_width = panel_width.saturating_sub(MARGIN_X * 2);
        let avail_height = spacer_height.saturating_sub(MARGIN_Y * 2);
        if avail_width < 4 || avail_height < 4 {
            return None;
        }

        let (img_width, img_height) = image::GenericImageView::dimensions(image);
        if img_width == 0 || img_height == 0 {
            return None;
        }

        // `height` below mirrors `Xywh::get_height`'s convention (raw aspect ratio, not yet
        // corrected for the ~2:1 cell width:height ratio) since that is what `draw_cover_ueberzug`
        // expects; the actual terminal rows this occupies is `height / 2` (used for fitting/centering).
        let max_width_for_height = avail_height.saturating_mul(2).saturating_mul(img_width) / img_height;
        let width = avail_width.min(max_width_for_height).max(1);
        let height = (width * img_height / img_width).max(1);
        let occupied_rows = (height / 2).max(1);

        let x = panel_x + MARGIN_X + avail_width.saturating_sub(width) / 2;
        let y = spacer_y + MARGIN_Y + avail_height.saturating_sub(occupied_rows) / 2;

        Some(Xywh {
            x_between_1_100: 0,
            y_between_1_100: 0,
            width_between_1_100: 0,
            x,
            y,
            width,
            height,
            // unused by direct x/y/width/height rendering; carry over an existing instance since
            // `AlignmentWrap`'s inner field is private to `termusiclib::xywh`.
            align: self.xywh.align.clone(),
        })
    }

    #[allow(clippy::cast_possible_truncation, clippy::unnecessary_wraps)]
    pub fn show_image(&mut self, img: &DynamicImage) -> Result<()> {
        #[allow(unused_variables)]
        let xywh = match self.player_cover_xywh(img) {
            Some(xywh) => xywh,
            None => self.xywh.update_size(img)?,
        };

        // error!("{:?}", self.viuer_supported);
        match self.viuer_supported {
            ViuerSupported::NotSupported => {
                #[cfg(all(feature = "cover-ueberzug", not(target_os = "windows")))]
                if let Some(instance) = self.ueberzug_instance.as_mut() {
                    let mut cache_file = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
                    cache_file.push("termusic");
                    if !cache_file.exists() {
                        std::fs::create_dir_all(&cache_file)?;
                    }
                    cache_file.push("termusic_cover.jpg");
                    img.save(&cache_file)?;
                    if !cache_file.exists() {
                        anyhow::bail!("cover file is not saved correctly");
                    }
                    if let Some(file) = cache_file.as_path().to_str() {
                        instance.draw_cover_ueberzug(file, &xywh, false)?;
                    }
                }
            }
            #[cfg(any(
                feature = "cover-viuer-iterm",
                feature = "cover-viuer-kitty",
                feature = "cover-viuer-sixel"
            ))]
            _ => {
                let config = viuer::Config {
                    transparent: true,
                    absolute_offset: true,
                    x: xywh.x as u16,
                    y: xywh.y as i16,
                    width: Some(xywh.width),
                    height: None,
                    // Force the specific protocol we probed for earlier
                    #[cfg(feature = "cover-viuer-iterm")]
                    use_iterm: self.viuer_supported == ViuerSupported::ITerm,
                    #[cfg(feature = "cover-viuer-kitty")]
                    use_kitty: self.viuer_supported == ViuerSupported::Kitty,
                    #[cfg(feature = "cover-viuer-sixel")]
                    use_sixel: self.viuer_supported == ViuerSupported::Sixel,
                    ..viuer::Config::default()
                };
                viuer::print(img, &config).context("viuer::print")?;
            }
        }

        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    fn clear_photo(&mut self) -> Result<()> {
        match self.viuer_supported {
            #[cfg(feature = "cover-viuer-kitty")]
            ViuerSupported::Kitty => {
                self.clear_image_viuer_kitty()
                    .context("clear_photo kitty")?;
                Self::remove_temp_files()?;
            }
            #[cfg(feature = "cover-viuer-iterm")]
            ViuerSupported::ITerm => {
                self.clear_image_viuer_kitty()
                    .context("clear_photo iterm")?;
                Self::remove_temp_files()?;
            }
            #[cfg(feature = "cover-viuer-sixel")]
            ViuerSupported::Sixel => {
                self.clear_image_viuer_kitty()
                    .context("clear_photo sixel")?;
                // sixel does not use temp-files, so no cleaning necessary
            }
            ViuerSupported::NotSupported => {
                #[cfg(all(feature = "cover-ueberzug", not(target_os = "windows")))]
                if let Some(instance) = self.ueberzug_instance.as_mut() {
                    instance.clear_cover_ueberzug()?;
                }
            }
        }
        Ok(())
    }

    #[cfg(any(
        feature = "cover-viuer-iterm",
        feature = "cover-viuer-kitty",
        feature = "cover-viuer-sixel"
    ))]
    fn clear_image_viuer_kitty(&mut self) -> Result<()> {
        use tuirealm::terminal::TerminalAdapter;

        write!(self.terminal.raw_mut().backend_mut(), "\x1b_Ga=d\x1b\\")?;
        self.terminal.raw_mut().backend_mut().flush()?;
        Ok(())
    }

    #[cfg(any(feature = "cover-viuer-iterm", feature = "cover-viuer-kitty"))]
    fn remove_temp_files() -> Result<()> {
        // Clean up temp files created by `viuer`'s kitty printer to avoid
        // possible freeze because of too many temp files in the temp folder.
        // Context: https://github.com/aome510/spotify-player/issues/148
        let tmp_dir = std::env::temp_dir();
        for path in (std::fs::read_dir(tmp_dir)?).flatten() {
            let path = path.path();
            if path.display().to_string().contains(".tmp.viuer") {
                std::fs::remove_file(path)?;
            }
        }

        Ok(())
    }
}
