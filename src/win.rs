// Copyright 2024 the Cartero authors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::app::CarteroApplication;
use glib::Object;
use glib::{subclass::types::ObjectSubclassIsExt, value::ToValue};
use gtk::gio::SettingsBindFlags;
use gtk::{
    gio::{self, Settings},
    glib,
    prelude::SettingsExtManual,
    WrapMode,
};

use gtk::prelude::ActionMapExt;
use gtk::prelude::SettingsExt;

mod imp {
    use std::collections::HashMap;

    use glib::GString;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use gtk::gio::ActionEntry;
    use gtk::Label;
    use gtk::Revealer;
    use gtk::StringObject;
    use isahc::RequestExt;
    use srtemplate::SrTemplate;
    use srtemplate::SrTemplateError;

    use crate::client::Request;
    use crate::client::RequestError;
    use crate::client::RequestMethod;
    use crate::client::Response;
    use crate::error::CarteroError;
    use crate::objects::Pair;
    use crate::widgets::*;
    use glib::subclass::InitializingObject;
    use gtk::{
        subclass::{
            application_window::ApplicationWindowImpl, widget::WidgetImpl, window::WindowImpl,
        },
        CompositeTemplate, TemplateChild,
    };

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/es/danirod/Cartero/main_window.ui")]
    pub struct CarteroWindow {
        #[template_child(id = "send")]
        pub send_button: TemplateChild<gtk::Button>,

        #[template_child]
        pub header_pane: TemplateChild<KeyValuePane>,

        #[template_child]
        pub variable_pane: TemplateChild<KeyValuePane>,

        #[template_child(id = "method")]
        pub request_method: TemplateChild<gtk::DropDown>,

        #[template_child(id = "url")]
        pub request_url: TemplateChild<gtk::Entry>,

        #[template_child]
        pub request_body: TemplateChild<sourceview5::View>,

        #[template_child]
        pub response: TemplateChild<ResponsePanel>,

        #[template_child]
        pub verbs_string_list: TemplateChild<gtk::StringList>,

        #[template_child]
        pub revealer: TemplateChild<Revealer>,

        #[template_child]
        pub revealer_text: TemplateChild<Label>,
    }

    #[gtk::template_callbacks]
    impl CarteroWindow {
        fn update_send_button_sensitivity(&self) {
            let empty = self.request_url.buffer().text().is_empty();
            self.send_button.set_sensitive(!empty);
        }

        #[template_callback]
        fn on_url_changed(&self) {
            self.update_send_button_sensitivity();
        }

        #[template_callback]
        fn on_close_revealer(&self) {
            self.hide_revealer()
        }

        fn request_method(&self) -> GString {
            self.request_method
                .selected_item()
                .unwrap()
                .downcast::<StringObject>()
                .unwrap()
                .string()
        }

        fn set_request_method(&self, rm: RequestMethod) {
            let verb_to_find = String::from(rm);
            let element_count = self.request_method.model().unwrap().n_items();
            let target_position = (0..element_count).find(|i| {
                if let Some(verb) = self.verbs_string_list.string(*i) {
                    if verb == verb_to_find {
                        return true;
                    }
                }
                false
            });
            if let Some(pos) = target_position {
                self.request_method.set_selected(pos);
            }
        }

        // Convert from a Request object into UI state
        fn assign_request(&self, req: &Request) {
            self.request_url.buffer().set_text(req.url.clone());
            self.set_request_method(req.method.clone());
            self.header_pane.set_entries(&req.headers);
            let body = String::from_utf8_lossy(&req.body);
            self.request_body.buffer().set_text(&body);
        }

        fn build_headers_map(
            &self,
            context: &SrTemplate,
        ) -> Result<HashMap<String, String>, CarteroError> {
            let vector = self.header_pane.get_entries();
            let possible_entries: Vec<Result<(String, String), SrTemplateError>> = vector
                .iter()
                .filter(|h| h.is_usable())
                .map(|h| {
                    let header_name = context.render(h.header_name())?;
                    let header_value = context.render(h.header_value())?;
                    Ok((header_name, header_value))
                })
                .collect();

            // TODO: I am pretty sure there is a way to collect() without having to do this.
            let mut headers = HashMap::new();
            for possible_entry in possible_entries {
                match possible_entry {
                    Ok((h, v)) => {
                        headers.insert(h.clone(), v.clone());
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(headers)
        }

        // Convert from UI state into a Request object
        fn extract_request(&self) -> Result<Request, CarteroError> {
            let variables = self.variable_pane.get_entries();
            let context = self.build_template_processor(&variables);

            let url = {
                let url = String::from(self.request_url.buffer().text());
                context.render(url)
            }?;

            let method = RequestMethod::try_from(self.request_method().as_str())?;
            let headers = self.build_headers_map(&context)?;

            let body = {
                let buffer = self.request_body.buffer();
                let (start, end) = buffer.bounds();
                let text = buffer.text(&start, &end, true);
                let content = context.render(text.as_str())?;
                Vec::from(content)
            };
            Ok(Request::new(url, method, headers, body))
        }

        fn build_template_processor(&self, variables: &[Pair]) -> SrTemplate {
            let context = SrTemplate::default();
            for variable in variables {
                if variable.is_usable() {
                    let header_name = variable.header_name();
                    let header_value = variable.header_value();
                    context.add_variable(header_name, &header_value);
                }
            }
            context
        }

        async fn trigger_open(&self) -> Result<(), CarteroError> {
            let obj = self.obj();
            let path = crate::widgets::open_file(&obj).await?;
            if let Some(path) = path {
                let contents = crate::file::read_file(&path)?;
                let request = crate::file::parse_toml(&contents)?;
                self.assign_request(&request);
            }
            Ok(())
        }

        async fn trigger_save(&self) -> Result<(), CarteroError> {
            let obj = self.obj();
            let path = crate::widgets::save_file(&obj).await?;
            if let Some(path) = path {
                let request = self.extract_request()?;
                let serialized_payload = crate::file::store_toml(&request)?;
                crate::file::write_file(&path, &serialized_payload)?;
            }
            Ok(())
        }

        fn perform_request(&self) -> Result<(), CarteroError> {
            let request = self.extract_request()?;
            let request_obj = isahc::Request::try_from(request)?;
            let mut response_obj = request_obj.send().map_err(RequestError::NetworkError)?;
            let response = Response::try_from(&mut response_obj)?;
            self.response.assign_from_response(&response);
            self.hide_revealer();
            Ok(())
        }

        pub(super) fn show_revealer(&self, str: &str) {
            self.revealer_text.set_label(str);
            self.revealer.set_reveal_child(true);
        }

        pub(super) fn hide_revealer(&self) {
            self.revealer.set_reveal_child(false);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CarteroWindow {
        const NAME: &'static str = "CarteroWindow";
        type Type = super::CarteroWindow;
        type ParentType = gtk::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            KeyValueRow::static_type();
            KeyValuePane::static_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CarteroWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let action_request = ActionEntry::builder("request")
                .activate(glib::clone!(@weak self as window => move |_, _, _| {
                    if let Err(e) = window.perform_request() {
                        let error_msg = format!("{}", e);
                        window.show_revealer(&error_msg)
                    }
                }))
                .build();
            let action_open = ActionEntry::builder("open")
                .activate(glib::clone!(@weak self as window => move |_, _, _| {
                    glib::spawn_future_local(glib::clone!(@weak window => async move {
                        if let Err(e) = window.trigger_open().await {
                            let error_msg = format!("{}", e);
                            window.show_revealer(&error_msg);
                        }
                    }));
                }))
                .build();
            let action_save = ActionEntry::builder("save")
                .activate(glib::clone!(@weak self as window => move |_, _, _| {
                    glib::spawn_future_local(glib::clone!(@weak window => async move {
                        if let Err(e) = window.trigger_save().await {
                            let error_msg = format!("{}", e);
                            window.show_revealer(&error_msg);
                        }
                    }));
                }))
                .build();

            let obj = self.obj();
            obj.add_action_entries([action_request, action_open, action_save]);
        }
    }

    impl WidgetImpl for CarteroWindow {
        fn show(&self) {
            self.parent_show();
            self.update_send_button_sensitivity();
        }
    }

    impl WindowImpl for CarteroWindow {}

    impl ApplicationWindowImpl for CarteroWindow {}
}

glib::wrapper! {
    pub struct CarteroWindow(ObjectSubclass<imp::CarteroWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl CarteroWindow {
    pub fn new(app: &CarteroApplication) -> Self {
        Object::builder().property("application", Some(app)).build()
    }

    pub fn assign_settings(&self, settings: &Settings) {
        let imp = &self.imp();

        let wrap = settings.create_action("body-wrap");
        self.add_action(&wrap);

        imp.response.get().assign_settings(&settings);

        let body = imp.request_body.get();
        settings
            .bind("body-wrap", &body, "wrap-mode")
            .flags(SettingsBindFlags::GET)
            .mapping(|variant, _| {
                let enabled = variant.get::<bool>().expect("The variant is not a boolean");
                let mode = match enabled {
                    true => WrapMode::Word,
                    false => WrapMode::None,
                };
                Some(mode.to_value())
            })
            .build();
    }

    pub fn show_revealer(&self, str: &str) {
        let imp = &self.imp();
        imp.show_revealer(str);
    }

    pub fn hide_revealer(&self) {
        let imp = &self.imp();
        imp.hide_revealer();
    }
}
