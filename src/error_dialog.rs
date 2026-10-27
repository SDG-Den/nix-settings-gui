use std::cell::RefCell;

use gtk::prelude::*;

thread_local! {
    static ERROR_DIALOG: RefCell<Option<gtk::MessageDialog>> = const { RefCell::new(None) };
}

pub(crate) fn show(parent: Option<&gtk::Window>, message: &str) {
    let reused = ERROR_DIALOG.with(|slot| {
        let slot = slot.borrow_mut();
        let Some(dialog) = slot.as_ref().filter(|dialog| dialog.is_visible()) else {
            return false;
        };
        if dialog.text().as_ref().map(|text| text.as_str()) != Some(message) {
            dialog.set_text(Some(message));
            dialog.present();
        }
        true
    });
    if reused {
        return;
    }

    let dialog = gtk::MessageDialog::builder()
        .modal(true)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text(message)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.connect_response(|dialog, _| dialog.close());
    ERROR_DIALOG.with(|slot| *slot.borrow_mut() = Some(dialog.clone()));
    dialog.present();
}
