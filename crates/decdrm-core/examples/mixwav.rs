//! Write a copy of a 16-bit I/Q WAV mixed down by N Hz (for the CLI, which cannot mix).
//! Usage: mixwav <in.wav> <hz> <out.wav>
use std::io::{Read, Write};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (inp, hz, outp) = (&a[1], a[2].parse::<f64>().unwrap(), &a[3]);
    let mut buf = Vec::new();
    std::fs::File::open(inp).unwrap().read_to_end(&mut buf).unwrap();
    assert_eq!(&buf[0..4], b"RIFF");
    let mut pos = 12;
    let (mut rate, mut ch) = (0u32, 0usize);
    let mut data = (0usize, 0usize);
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos+4];
        let len = u32::from_le_bytes([buf[pos+4],buf[pos+5],buf[pos+6],buf[pos+7]]) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => { ch = u16::from_le_bytes([buf[body+2],buf[body+3]]) as usize;
                         rate = u32::from_le_bytes([buf[body+4],buf[body+5],buf[body+6],buf[body+7]]); }
            b"data" => data = (body, len.min(buf.len()-body)),
            _ => {}
        }
        pos = body + len + (len & 1);
    }
    let (body, len) = data;
    let n = len / 2 / ch;
    let mut out = Vec::with_capacity(len);
    let w = -2.0 * std::f64::consts::PI * hz / f64::from(rate);
    let mut ph = 0.0f64;
    for i in 0..n {
        let re = f64::from(i16::from_le_bytes([buf[body+2*i*ch], buf[body+2*i*ch+1]])) / 32768.0;
        let im = f64::from(i16::from_le_bytes([buf[body+2*i*ch+2], buf[body+2*i*ch+3]])) / 32768.0;
        let (c, s) = (ph.cos(), ph.sin());
        let (r2, i2) = (re*c - im*s, re*s + im*c);
        out.extend_from_slice(&((r2 * 32767.0) as i16).to_le_bytes());
        out.extend_from_slice(&((i2 * 32767.0) as i16).to_le_bytes());
        ph += w;
        if ph > std::f64::consts::PI { ph -= 2.0*std::f64::consts::PI; }
        else if ph < -std::f64::consts::PI { ph += 2.0*std::f64::consts::PI; }
    }
    // Minimal WAV header: 16-bit, `ch` channels, `rate`.
    let mut h = Vec::new();
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((36 + out.len()) as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&(ch as u16).to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
    h.extend_from_slice(&((ch * 2) as u16).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(out.len() as u32).to_le_bytes());
    let mut f = std::fs::File::create(outp).unwrap();
    f.write_all(&h).unwrap();
    f.write_all(&out).unwrap();
    println!("wrote {outp}: mixed down {hz} Hz, {n} frames");
}
