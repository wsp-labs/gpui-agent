use gpui::{
    AnyWindowHandle, Context, Entity, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, PlatformInput, StyleRefinement, Window, div, point, prelude::*, px, rgb, size,
};

struct Footer;

impl Render for Footer {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        println!("FOOTER RENDER");
        div().id("footer").child("cached footer text")
    }
}

struct Spike {
    count: usize,
    footer: Entity<Footer>,
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .id("btn")
                    .w(px(120.))
                    .h(px(40.))
                    .bg(rgb(0x3060ff))
                    .hover(|s| s.bg(rgb(0x4070ff)))
                    .child("Increment")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.count += 1;
                        cx.notify();
                    })),
            )
            .child(div().child(format!("count: {}", self.count)))
            .child(
                div()
                    .id("list")
                    .h(px(40.))
                    .overflow_y_scroll()
                    .child(div().h(px(30.)).child("row one"))
                    .child(div().h(px(30.)).child("row two, clipped")),
            )
            .child(self.footer.clone().cached(StyleRefinement::default()))
    }
}

fn click(cx: &mut HeadlessAppContext, w: AnyWindowHandle, x: f32, y: f32) {
    cx.update_window(w, |_, window, cx| {
        let position = point(px(x), px(y));
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
}

fn dump(cx: &mut HeadlessAppContext, w: AnyWindowHandle, label: &str) {
    cx.update_window(w, |_, window, _| {
        println!("== {label}: {} nodes, {} texts", window.driver_nodes().len(), window.driver_texts().len());
        for (i, n) in window.driver_nodes().iter().enumerate() {
            let short = n.type_name.rsplit("::").next().unwrap_or(n.type_name);
            let b = n.bounds;
            let v = n.visible_bounds;
            println!(
                "  #{i} parent={:?} {short} id={:?} at={:?} visible={:?} flags={:?} src={}",
                n.parent,
                n.element_id,
                b,
                v.size,
                n.flags,
                n.source.map(|s| format!("{}:{}", s.file(), s.line())).unwrap_or_default(),
            );
        }
        for t in window.driver_texts() {
            println!(
                "  text {:?} at={:?} visible={:?}",
                t.text.as_ref(), t.bounds.origin, t.visible_bounds.size
            );
        }
    })
    .unwrap();
}

fn main() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), || {
        gpui_platform::current_headless_renderer()
    });
    let handle = cx
        .open_window(size(px(400.), px(300.)), |_, cx| {
            let footer = cx.new(|_| Footer);
            cx.new(|_| Spike { count: 0, footer })
        })
        .unwrap();
    cx.run_until_parked();
    let w: AnyWindowHandle = handle.into();
    dump(&mut cx, w, "off");
    cx.update_window(w, |_, window, _| window.set_driver_enabled(true)).unwrap();
    cx.run_until_parked();
    dump(&mut cx, w, "on");
    click(&mut cx, w, 80., 40.);
    dump(&mut cx, w, "after click");
    println!("-- notify only");
    cx.update(|cx| handle.update(cx, |v, _, cx| { v.count += 10; cx.notify(); })).unwrap();
    cx.run_until_parked();
    dump(&mut cx, w, "after notify");
}
