use gpui::{
    AnyWindowHandle, Context, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, PlatformInput, Window, div, point, prelude::*, px, rgb, size,
};

struct Spike {
    count: usize,
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
                    .child("Increment")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.count += 1;
                        cx.notify();
                    })),
            )
            .child(div().id("label").child(format!("count: {}", self.count)))
    }
}

fn click(cx: &mut HeadlessAppContext, w: AnyWindowHandle) {
    cx.update_window(w, |_, window, cx| {
        let position = point(px(80.), px(40.));
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

fn main() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), || {
        gpui_platform::current_headless_renderer()
    });
    let handle = cx
        .open_window(size(px(400.), px(300.)), |_, cx| {
            cx.new(|_| Spike { count: 0 })
        })
        .unwrap();
    cx.run_until_parked();
    let w: AnyWindowHandle = handle.into();
    cx.capture_screenshot(w)
        .unwrap()
        .save("/tmp/spike-before.png")
        .unwrap();
    click(&mut cx, w);
    click(&mut cx, w);
    let count = cx.update(|cx| handle.read(cx).map(|v| v.count)).unwrap();
    println!("SPIKE headless count={count}");
    cx.update_window(w, |_, window, _| {
        println!("SPIKE headless a11y_json={:?}", window.debug_a11y_tree_json());
    })
    .unwrap();
    let after = cx.capture_screenshot(w).unwrap();
    after.save("/tmp/spike-after.png").unwrap();
    println!("SPIKE headless png {}x{}", after.width(), after.height());
}
