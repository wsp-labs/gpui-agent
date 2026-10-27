//! A GPUI app with the things gpui-agent has to handle: buttons, a text field, a long
//! scrolling list, a cached view, an overlay that covers a button, actions and probes.

mod input;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, StyleRefinement, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use gpui_platform::application;
use input::TextInput;

actions!(testapp, [Reset, Increment]);

struct Footer;

impl Render for Footer {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("footer")
            .text_color(rgb(0x888888))
            .child("footer: a cached view")
    }
}

struct TestApp {
    count: usize,
    selected: Option<usize>,
    overlay: bool,
    status: String,
    file: String,
    name: Entity<TextInput>,
    footer: Entity<Footer>,
    focus_handle: FocusHandle,
}

impl Focusable for TestApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(6.))
        .bg(rgb(0x3060ff))
        .hover(|style| style.bg(rgb(0x4070ff)))
        .rounded(px(4.))
        .child(label)
}

impl Render for TestApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.name.read(cx).text();
        div()
            .id("root")
            .key_context("TestApp")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Reset, _, cx| {
                this.count = 0;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Increment, _, cx| {
                this.count += 1;
                cx.notify();
            }))
            .size_full()
            .relative()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xffffff))
            .text_size(px(14.))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(18.)).child("gpui-agent test app"))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(button("increment", "Increment").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.count += 1;
                            cx.notify();
                        },
                    )))
                    .child(
                        button("reset", "Reset").on_click(cx.listener(|this, _, _, cx| {
                            this.count = 0;
                            cx.notify();
                        })),
                    )
                    .child(div().child(format!("count: {}", self.count))),
            )
            .child(
                div()
                    .w(px(320.))
                    .text_color(rgb(0x111111))
                    .child(self.name.clone()),
            )
            .child(div().child(if name.is_empty() {
                "hello, nobody".to_string()
            } else {
                format!("hello, {name}")
            }))
            .child(button("show-overlay", "Show overlay").on_click(cx.listener(
                |this, _, _, cx| {
                    this.overlay = true;
                    cx.notify();
                },
            )))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(button("delete", "Delete everything").on_click(cx.listener(
                        |_, _, window, cx| {
                            let answer = window.prompt(
                                gpui::PromptLevel::Warning,
                                "Delete everything?",
                                Some("This cannot be undone."),
                                &["Delete", "Cancel"],
                                cx,
                            );
                            cx.spawn(async move |this, cx| {
                                let status = match answer.await {
                                    Ok(0) => "deleted",
                                    _ => "kept",
                                };
                                this.update(cx, |this, cx| {
                                    this.status = status.into();
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        },
                    )))
                    .child(div().child(format!("status: {}", self.status))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        button("open", "Open file").on_click(cx.listener(|_, _, _, cx| {
                            let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: None,
                            });
                            cx.spawn(async move |this, cx| {
                                let file = match paths.await {
                                    Ok(Ok(Some(paths))) => paths
                                        .first()
                                        .map(|path| format!("opened {}", path.display()))
                                        .unwrap_or_default(),
                                    _ => "open cancelled".into(),
                                };
                                this.update(cx, |this, cx| {
                                    this.file = file;
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        })),
                    )
                    .child(
                        button("save", "Save as").on_click(cx.listener(|_, _, _, cx| {
                            let path = cx.prompt_for_new_path(
                                std::path::Path::new("/tmp"),
                                Some("notes.txt"),
                            );
                            cx.spawn(async move |this, cx| {
                                let file = match path.await {
                                    Ok(Ok(Some(path))) => format!("saved {}", path.display()),
                                    _ => "save cancelled".into(),
                                };
                                this.update(cx, |this, cx| {
                                    this.file = file;
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        })),
                    )
                    .child(div().child(format!("file: {}", self.file))),
            )
            .child(div().child(match self.selected {
                Some(row) => format!("selected: row {row}"),
                None => "selected: none".to_string(),
            }))
            .child(
                div()
                    .id("rows")
                    .h(px(150.))
                    .w(px(320.))
                    .bg(rgb(0x2a2a2a))
                    .overflow_y_scroll()
                    .children((1..=50).map(|row| {
                        div()
                            .id(("row", row))
                            .px(px(8.))
                            .h(px(24.))
                            .hover(|style| style.bg(rgb(0x3a3a3a)))
                            .when(self.selected == Some(row), |div| div.bg(rgb(0x30507f)))
                            .child(format!("row {row}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(row);
                                cx.notify();
                            }))
                    })),
            )
            .child(self.footer.clone().cached(StyleRefinement::default()))
            .when(self.overlay, |root| {
                root.child(
                    div()
                        .id("overlay")
                        .occlude()
                        .absolute()
                        .top(px(40.))
                        .left(px(8.))
                        .w(px(300.))
                        .h(px(90.))
                        .bg(rgb(0x553333))
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child("An overlay over the buttons")
                        .child(button("dismiss", "Dismiss").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.overlay = false;
                                cx.notify();
                            },
                        ))),
                )
            })
    }
}

fn main() {
    application().run(|cx: &mut App| {
        gpui_agent::init(cx);
        input::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-r", Reset, Some("TestApp")),
            KeyBinding::new("ctrl-i", Increment, Some("TestApp")),
        ]);
        let bounds = Bounds::centered(None, size(px(480.), px(520.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: false,
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("gpui-agent testapp".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    let name = cx.new(|cx| TextInput::new("Your name", cx));
                    let footer = cx.new(|_| Footer);
                    cx.new(|cx| {
                        cx.observe(&name, |_, _, cx| cx.notify()).detach();
                        TestApp {
                            count: 0,
                            selected: None,
                            overlay: false,
                            status: "nothing deleted".into(),
                            file: "none".into(),
                            name,
                            footer,
                            focus_handle: cx.focus_handle(),
                        }
                    })
                },
            )
            .unwrap();
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle, cx);
            })
            .unwrap();

        let app = window;
        gpui_agent::probe(cx, "count", move |cx| {
            serde_json::json!(app.read(cx).map(|view| view.count).ok())
        });
        gpui_agent::probe(cx, "selected", move |cx| {
            serde_json::json!(app.read(cx).ok().and_then(|view| view.selected))
        });
        gpui_agent::probe(cx, "name", move |cx| {
            serde_json::json!(
                app.read(cx)
                    .ok()
                    .map(|view| view.name.read(cx).text().to_string())
            )
        });
    });
}
