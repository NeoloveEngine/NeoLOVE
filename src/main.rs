mod hierachy;

use minifb::{Key, KeyRepeat, MouseMode, Window, WindowOptions};

fn main() {
    let mut window = Window::new(
        "minifb pixels (esc to quit)",
        640,
        360,
        WindowOptions {
            resize: true,
            ..WindowOptions::default()
        },
    )
        .expect("unable to open window");

    // 0 = no waiting (max fps, max cpu). default is already high.
    window.set_target_fps(0);

    let mut buffer: Vec<u32> = Vec::new();
    let mut frame: u32 = 0;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        // a) get screen size (in pixels)
        let (w, h) = window.get_size();
        let w = w.max(1);
        let h = h.max(1);

        // b) ensure we have a big enough pixel buffer
        if buffer.len() != w * h {
            buffer.resize(w * h, 0);
        }

        // c) read inputs (example: mouse position affects the picture)
        let (mx, my) = window
            .get_mouse_pos(MouseMode::Clamp)
            .unwrap_or((0.0, 0.0));
        let mx = mx as i32;
        let my = my as i32;

        // d) write pixels (0x00RRGGBB)
        // this redraws the whole frame every loop
        let t = frame;
        for y in 0..h {
            let yy = y as i32;
            for x in 0..w {
                let xx = x as i32;

                let r = ((x as u32).wrapping_add(t) & 0xff) as u32;
                let g = ((y as u32).wrapping_add(t.wrapping_mul(2)) & 0xff) as u32;
                let mut b = ((t.wrapping_mul(3)) & 0xff) as u32;

                // simple "spotlight" around the mouse
                let dx = (xx - mx).abs() as u32;
                let dy = (yy - my).abs() as u32;
                if dx < 40 && dy < 40 {
                    b = 255;
                }

                buffer[y * w + x] = (r << 16) | (g << 8) | b;
            }
        }

        // e) present the buffer (also pumps window events)
        window
            .update_with_buffer(&buffer, w, h)
            .expect("update failed");

        // example: press space to reset animation
        if window.is_key_pressed(Key::Space, KeyRepeat::No) {
            frame = 0;
        } else {
            frame = frame.wrapping_add(1);
        }
    }
}
