use maple_exp::reader::*;

fn load(name: &str) -> Frame {
    let dec = png::Decoder::new(std::fs::File::open(format!("tests/frames/{name}.png")).unwrap());
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut bgra = Vec::with_capacity(w * h * 4);
    for p in buf[..w * h * ch].chunks(ch) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    Frame { w, h, bgra }
}

#[test]
fn reads_all_sample_bars() {
    let t = Templates::builtin();
    for n in ["exp_full-1280x720", "exp_full-1920x1080", "exp_win-1024x768", "exp_win-1280x720", "exp_win-1920x1080", "exp_win-1920x1200"] {
        let f = load(n);
        let (_, r) = find_and_read(&t, &f).unwrap_or_else(|| panic!("{n}: not read"));
        assert_eq!(r, Reading { exp: Some(68601), pct: 71.68 }, "{n}");
    }
}
