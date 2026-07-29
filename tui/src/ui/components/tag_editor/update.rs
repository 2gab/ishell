use crate::ui::Model;
use crate::ui::ids::{Id, IdTagEditor};
use crate::ui::msg::{TEMsg, TFMsg};

impl Model {
    pub fn update_tageditor(&mut self, msg: TEMsg) {
        match msg {
            TEMsg::Open(path) => {
                self.mount_tageditor(&path);
            }
            TEMsg::Close => {
                if let Some(s) = self.tageditor.song.take() {
                    if self.tageditor.has_changed
                        && self
                            .current_track_lyric
                            .as_ref()
                            .is_some_and(|v| v.for_track == s.path())
                    {
                        self.lyric_reload_from_file();
                    }
                    self.tageditor.has_changed = false;
                    self.new_library_reload_and_focus(s.into_path());
                }
                self.umount_tageditor();
            }
            TEMsg::Save => {
                if let Err(e) = self.te_save_tag() {
                    self.mount_error_popup(e.context("rename song by tag"));
                }
            }
            TEMsg::Focus(msg) => self.update_tag_editor_focus(msg),
        }
    }

    fn update_tag_editor_focus(&mut self, msg: TFMsg) {
        match msg {
            TFMsg::TextareaLyricBlurDown | TFMsg::InputTitleBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::InputArtist))
                    .ok();
            }
            TFMsg::InputArtistBlurDown | TFMsg::InputAlbumBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::InputTitle))
                    .ok();
            }
            TFMsg::InputTitleBlurDown | TFMsg::InputGenreBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::InputAlbum))
                    .ok();
            }
            TFMsg::InputAlbumBlurDown | TFMsg::TableLyricOptionsBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::InputGenre))
                    .ok();
            }
            TFMsg::InputGenreBlurDown | TFMsg::SelectLyricBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::TableLyricOptions))
                    .ok();
            }
            TFMsg::TableLyricOptionsBlurDown | TFMsg::CounterDeleteBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::SelectLyric))
                    .ok();
            }
            TFMsg::SelectLyricBlurDown | TFMsg::CounterSaveBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::CounterDelete))
                    .ok();
            }
            TFMsg::CounterDeleteBlurDown | TFMsg::TextareaLyricBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::CounterSave))
                    .ok();
            }
            TFMsg::CounterSaveBlurDown | TFMsg::InputArtistBlurUp => {
                self.app
                    .active(&Id::TagEditor(IdTagEditor::TextareaLyric))
                    .ok();
            }
        }
    }
}
