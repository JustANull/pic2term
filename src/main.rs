extern crate ansi_term;
extern crate clap;
extern crate conv;
extern crate image;
extern crate itertools;
extern crate termsize;

use ansi_term::{ANSIStrings, Colour};
use clap::{App, Arg};
use conv::{UnwrapOrSaturate, ValueFrom};
use image::{imageops, FilterType, RgbImage};
use itertools::Itertools;
use termsize::Size;

use std::ops::Add;

fn determine_size(aspect: f32, desired_w: Option<u16>, desired_h: Option<u16>) -> Option<(u16, u16)> {
    // To note, we're outputting with double density vertically due to the
    // Unicode bottom-half character, so we need to consider that in size
    // calculations if the user provided a height.
    let desired_h = desired_h.map(|n| n * 2);

    if let Some(desired_w) = desired_w {
        if let Some(desired_h) = desired_h {
            Some((desired_w, desired_h))
        } else {
            // Width is known, height is not. Match height to the aspect ratio
            Some((desired_w, (desired_w as f32 / aspect) as u16))
        }
    } else {
        if let Some(desired_h) = desired_h {
            // Height is known, width is not. Match width to the aspect ratio
            Some(((desired_h as f32 * aspect) as u16, desired_h))
        } else {
            // Width and height are unknown
            match termsize::get() {
                Some(Size { rows: h, cols: w }) => {
                    // Our terminal is virtually twice as tall as we otherwise believe it to be.
                    let h = h * 2;

                    // Take the smaller dimension and scale the other to fit
                    if w < h {
                        let rescaled_h = (w as f32 / aspect) as u16;
                        if rescaled_h > h {
                            let scale = h as f32 / rescaled_h as f32;
                            Some(((w as f32 * scale) as u16, h))
                        } else {
                            Some((w, rescaled_h))
                        }
                    } else { // h <= w
                        let rescaled_w = (h as f32 * aspect) as u16;
                        if rescaled_w > w {
                            let scale = w as f32 / rescaled_w as f32;
                            Some((w, (h as f32 * scale) as u16))
                        } else {
                            Some((rescaled_w, h))
                        }
                    }
                },
                None => None
            }
        }
    }
}
fn determine_filter(filter_str: &str) -> FilterType {
    match filter_str {
        "nearest" => FilterType::Nearest,
        "triangle" => FilterType::Triangle,
        "gaussian" => FilterType::Gaussian,
        "catmullrom" => FilterType::CatmullRom,
        "lanczos3" => FilterType::Lanczos3,
        _ => unreachable!(),
    }
}
fn is_u16(s: String) -> Result<(), String> {
    s.parse::<u16>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn dither(img: RgbImage, colors: &[[u8; 3]]) -> Vec<usize> {
    // The magic number is 3
    let (width, height) = img.dimensions();
    let mut res = Vec::with_capacity(width as usize * height as usize);
    let mut raw = img.into_raw();

    for y in 0..height {
        for x in 0..width {
            let cur_idx = 3 * (x + y * width) as usize;

            let (dithered_idx, diff) = {
                let cur_pixel = &raw[cur_idx..cur_idx + 3];
                let (dithered_idx, dithered) = colors.iter().enumerate()
                    .min_by_key(|&(_, col)| {
                        cur_pixel.into_iter()
                            .zip(col)
                            .map(|(a, b)| *a as isize - *b as isize)
                            .map(|n| n * n)
                            .fold(0, Add::add)
                    }).unwrap();
                let diff = cur_pixel.into_iter()
                    .zip(dithered)
                    .map(|(a, b)| *a as i16 - *b as i16)
                    .collect::<Vec<i16>>();

                (dithered_idx, diff)
            };

            res.push(dithered_idx);

            // This only supports dithering algorithms which modify ahead
            macro_rules! pix_add {
                ($x:expr, $y:expr, $numerator:expr, $denominator:expr) => {{
                    if $x > 0 && $x < width {
                        if $y < height {
                            let idx = 3 * ($x + $y * width) as usize;
                            for (channel, offset) in raw[idx..idx + 3].iter_mut().zip(&diff) {
                                *channel = u8::value_from(*channel as i16 + *offset * $numerator / $denominator).unwrap_or_saturate();

                            }
                        }
                    }
                }};
            };

            pix_add!(x + 1, y, 7, 48);
            pix_add!(x + 2, y, 5, 48);
            pix_add!(x - 2, y + 1, 3, 48);
            pix_add!(x - 1, y + 1, 5, 48);
            pix_add!(x, y + 1, 7, 48);
            pix_add!(x + 1, y + 1, 5, 48);
            pix_add!(x + 2, y + 1, 3, 48);
            pix_add!(x - 2, y + 2, 1, 48);
            pix_add!(x - 1, y + 2, 3, 48);
            pix_add!(x, y + 2, 5, 48);
            pix_add!(x + 1, y + 2, 3, 48);
            pix_add!(x + 2, y + 2, 1, 48);
        }
    }

    res
}

fn main() {
    let matches = App::new("pic2term")
        .version("0.0.2")
        .author("JustANull <reid.levenick@gmail.com>")
        .about("Renders images to the terminal with Unicode characters")
        .arg(Arg::with_name("width")
             .long("width")
             .help("The width (in columns) to resize the image to")
             .value_name("WIDTH")
             .validator(is_u16))
        .arg(Arg::with_name("height")
             .long("height")
             .help("The height (in rows) to resize the image to")
             .value_name("HEIGHT")
             .validator(is_u16))
        .arg(Arg::with_name("filter")
             .long("filter")
             .help("The filter to use when downscaling the image")
             .possible_values(&["nearest", "triangle", "gaussian", "catmullrom", "lanczos3"])
             .default_value("nearest")
             .value_name("FILTER"))
        .arg(Arg::with_name("file")
             .index(1)
             .help("The file to render")
             .required(true)
             .value_name("FILE"))
        .get_matches();

    let file = matches.value_of("file").unwrap();
    let img = image::open(file).expect("The file provided should actually exist").to_rgb();

    let (w, h) = determine_size(img.width() as f32 / img.height() as f32,
                                matches.value_of("width").map(str::parse).map(Result::unwrap),
                                matches.value_of("height").map(str::parse).map(Result::unwrap))
        .expect("Unable to determine terminal size, pass --width or --height flags");
    let filter = determine_filter(matches.value_of("filter").unwrap());

    let indices = dither(imageops::resize(&img, w as u32, h as u32, filter), &ANSI_COLORS);
    let rows = indices.chunks(w as usize).chunks_lazy(2);
    for mut pair in rows.into_iter() {
        let upper = pair.next().unwrap();
        let lower = pair.next();

        println!("{}", ANSIStrings(&match lower {
            Some(lower) => (0..w as usize).map(|idx| {
                Colour::Fixed(lower[idx] as u8).on(Colour::Fixed(upper[idx] as u8)).paint("\u{2584}")
            }).collect::<Vec<_>>(),
            None => (0..w as usize).map(|idx| {
                Colour::Fixed(upper[idx] as u8).paint("\u{2580}")
            }).collect::<Vec<_>>(),
        }));
    }
}

static ANSI_COLORS: [[u8; 3]; 256] = [
    [0x00, 0x00, 0x00], [0x80, 0x00, 0x00], [0x00, 0x80, 0x00],
    [0x80, 0x80, 0x00], [0x00, 0x00, 0x80], [0x80, 0x00, 0x80],
    [0x00, 0x80, 0x80], [0xc0, 0xc0, 0xc0], [0x80, 0x80, 0x80],
    [0xff, 0x00, 0x00], [0x00, 0xff, 0x00], [0xff, 0xff, 0x00],
    [0x00, 0x00, 0xff], [0xff, 0x00, 0xff], [0x00, 0xff, 0xff],
    [0xff, 0xff, 0xff], [0x00, 0x00, 0x00], [0x00, 0x00, 0x5f],
    [0x00, 0x00, 0x87], [0x00, 0x00, 0xaf], [0x00, 0x00, 0xd7],
    [0x00, 0x00, 0xff], [0x00, 0x5f, 0x00], [0x00, 0x5f, 0x5f],
    [0x00, 0x5f, 0x87], [0x00, 0x5f, 0xaf], [0x00, 0x5f, 0xd7],
    [0x00, 0x5f, 0xff], [0x00, 0x87, 0x00], [0x00, 0x87, 0x5f],
    [0x00, 0x87, 0x87], [0x00, 0x87, 0xaf], [0x00, 0x87, 0xd7],
    [0x00, 0x87, 0xff], [0x00, 0xaf, 0x00], [0x00, 0xaf, 0x5f],
    [0x00, 0xaf, 0x87], [0x00, 0xaf, 0xaf], [0x00, 0xaf, 0xd7],
    [0x00, 0xaf, 0xff], [0x00, 0xd7, 0x00], [0x00, 0xd7, 0x5f],
    [0x00, 0xd7, 0x87], [0x00, 0xd7, 0xaf], [0x00, 0xd7, 0xd7],
    [0x00, 0xd7, 0xff], [0x00, 0xff, 0x00], [0x00, 0xff, 0x5f],
    [0x00, 0xff, 0x87], [0x00, 0xff, 0xaf], [0x00, 0xff, 0xd7],
    [0x00, 0xff, 0xff], [0x5f, 0x00, 0x00], [0x5f, 0x00, 0x5f],
    [0x5f, 0x00, 0x87], [0x5f, 0x00, 0xaf], [0x5f, 0x00, 0xd7],
    [0x5f, 0x00, 0xff], [0x5f, 0x5f, 0x00], [0x5f, 0x5f, 0x5f],
    [0x5f, 0x5f, 0x87], [0x5f, 0x5f, 0xaf], [0x5f, 0x5f, 0xd7],
    [0x5f, 0x5f, 0xff], [0x5f, 0x87, 0x00], [0x5f, 0x87, 0x5f],
    [0x5f, 0x87, 0x87], [0x5f, 0x87, 0xaf], [0x5f, 0x87, 0xd7],
    [0x5f, 0x87, 0xff], [0x5f, 0xaf, 0x00], [0x5f, 0xaf, 0x5f],
    [0x5f, 0xaf, 0x87], [0x5f, 0xaf, 0xaf], [0x5f, 0xaf, 0xd7],
    [0x5f, 0xaf, 0xff], [0x5f, 0xd7, 0x00], [0x5f, 0xd7, 0x5f],
    [0x5f, 0xd7, 0x87], [0x5f, 0xd7, 0xaf], [0x5f, 0xd7, 0xd7],
    [0x5f, 0xd7, 0xff], [0x5f, 0xff, 0x00], [0x5f, 0xff, 0x5f],
    [0x5f, 0xff, 0x87], [0x5f, 0xff, 0xaf], [0x5f, 0xff, 0xd7],
    [0x5f, 0xff, 0xff], [0x87, 0x00, 0x00], [0x87, 0x00, 0x5f],
    [0x87, 0x00, 0x87], [0x87, 0x00, 0xaf], [0x87, 0x00, 0xd7],
    [0x87, 0x00, 0xff], [0x87, 0x5f, 0x00], [0x87, 0x5f, 0x5f],
    [0x87, 0x5f, 0x87], [0x87, 0x5f, 0xaf], [0x87, 0x5f, 0xd7],
    [0x87, 0x5f, 0xff], [0x87, 0x87, 0x00], [0x87, 0x87, 0x5f],
    [0x87, 0x87, 0x87], [0x87, 0x87, 0xaf], [0x87, 0x87, 0xd7],
    [0x87, 0x87, 0xff], [0x87, 0xaf, 0x00], [0x87, 0xaf, 0x5f],
    [0x87, 0xaf, 0x87], [0x87, 0xaf, 0xaf], [0x87, 0xaf, 0xd7],
    [0x87, 0xaf, 0xff], [0x87, 0xd7, 0x00], [0x87, 0xd7, 0x5f],
    [0x87, 0xd7, 0x87], [0x87, 0xd7, 0xaf], [0x87, 0xd7, 0xd7],
    [0x87, 0xd7, 0xff], [0x87, 0xff, 0x00], [0x87, 0xff, 0x5f],
    [0x87, 0xff, 0x87], [0x87, 0xff, 0xaf], [0x87, 0xff, 0xd7],
    [0x87, 0xff, 0xff], [0xaf, 0x00, 0x00], [0xaf, 0x00, 0x5f],
    [0xaf, 0x00, 0x87], [0xaf, 0x00, 0xaf], [0xaf, 0x00, 0xd7],
    [0xaf, 0x00, 0xff], [0xaf, 0x5f, 0x00], [0xaf, 0x5f, 0x5f],
    [0xaf, 0x5f, 0x87], [0xaf, 0x5f, 0xaf], [0xaf, 0x5f, 0xd7],
    [0xaf, 0x5f, 0xff], [0xaf, 0x87, 0x00], [0xaf, 0x87, 0x5f],
    [0xaf, 0x87, 0x87], [0xaf, 0x87, 0xaf], [0xaf, 0x87, 0xd7],
    [0xaf, 0x87, 0xff], [0xaf, 0xaf, 0x00], [0xaf, 0xaf, 0x5f],
    [0xaf, 0xaf, 0x87], [0xaf, 0xaf, 0xaf], [0xaf, 0xaf, 0xd7],
    [0xaf, 0xaf, 0xff], [0xaf, 0xd7, 0x00], [0xaf, 0xd7, 0x5f],
    [0xaf, 0xd7, 0x87], [0xaf, 0xd7, 0xaf], [0xaf, 0xd7, 0xd7],
    [0xaf, 0xd7, 0xff], [0xaf, 0xff, 0x00], [0xaf, 0xff, 0x5f],
    [0xaf, 0xff, 0x87], [0xaf, 0xff, 0xaf], [0xaf, 0xff, 0xd7],
    [0xaf, 0xff, 0xff], [0xd7, 0x00, 0x00], [0xd7, 0x00, 0x5f],
    [0xd7, 0x00, 0x87], [0xd7, 0x00, 0xaf], [0xd7, 0x00, 0xd7],
    [0xd7, 0x00, 0xff], [0xd7, 0x5f, 0x00], [0xd7, 0x5f, 0x5f],
    [0xd7, 0x5f, 0x87], [0xd7, 0x5f, 0xaf], [0xd7, 0x5f, 0xd7],
    [0xd7, 0x5f, 0xff], [0xd7, 0x87, 0x00], [0xd7, 0x87, 0x5f],
    [0xd7, 0x87, 0x87], [0xd7, 0x87, 0xaf], [0xd7, 0x87, 0xd7],
    [0xd7, 0x87, 0xff], [0xd7, 0xaf, 0x00], [0xd7, 0xaf, 0x5f],
    [0xd7, 0xaf, 0x87], [0xd7, 0xaf, 0xaf], [0xd7, 0xaf, 0xd7],
    [0xd7, 0xaf, 0xff], [0xd7, 0xd7, 0x00], [0xd7, 0xd7, 0x5f],
    [0xd7, 0xd7, 0x87], [0xd7, 0xd7, 0xaf], [0xd7, 0xd7, 0xd7],
    [0xd7, 0xd7, 0xff], [0xd7, 0xff, 0x00], [0xd7, 0xff, 0x5f],
    [0xd7, 0xff, 0x87], [0xd7, 0xff, 0xaf], [0xd7, 0xff, 0xd7],
    [0xd7, 0xff, 0xff], [0xff, 0x00, 0x00], [0xff, 0x00, 0x5f],
    [0xff, 0x00, 0x87], [0xff, 0x00, 0xaf], [0xff, 0x00, 0xd7],
    [0xff, 0x00, 0xff], [0xff, 0x5f, 0x00], [0xff, 0x5f, 0x5f],
    [0xff, 0x5f, 0x87], [0xff, 0x5f, 0xaf], [0xff, 0x5f, 0xd7],
    [0xff, 0x5f, 0xff], [0xff, 0x87, 0x00], [0xff, 0x87, 0x5f],
    [0xff, 0x87, 0x87], [0xff, 0x87, 0xaf], [0xff, 0x87, 0xd7],
    [0xff, 0x87, 0xff], [0xff, 0xaf, 0x00], [0xff, 0xaf, 0x5f],
    [0xff, 0xaf, 0x87], [0xff, 0xaf, 0xaf], [0xff, 0xaf, 0xd7],
    [0xff, 0xaf, 0xff], [0xff, 0xd7, 0x00], [0xff, 0xd7, 0x5f],
    [0xff, 0xd7, 0x87], [0xff, 0xd7, 0xaf], [0xff, 0xd7, 0xd7],
    [0xff, 0xd7, 0xff], [0xff, 0xff, 0x00], [0xff, 0xff, 0x5f],
    [0xff, 0xff, 0x87], [0xff, 0xff, 0xaf], [0xff, 0xff, 0xd7],
    [0xff, 0xff, 0xff], [0x08, 0x08, 0x08], [0x12, 0x12, 0x12],
    [0x1c, 0x1c, 0x1c], [0x26, 0x26, 0x26], [0x30, 0x30, 0x30],
    [0x3a, 0x3a, 0x3a], [0x44, 0x44, 0x44], [0x4e, 0x4e, 0x4e],
    [0x58, 0x58, 0x58], [0x60, 0x60, 0x60], [0x66, 0x66, 0x66],
    [0x76, 0x76, 0x76], [0x80, 0x80, 0x80], [0x8a, 0x8a, 0x8a],
    [0x94, 0x94, 0x94], [0x9e, 0x9e, 0x9e], [0xa8, 0xa8, 0xa8],
    [0xb2, 0xb2, 0xb2], [0xbc, 0xbc, 0xbc], [0xc6, 0xc6, 0xc6],
    [0xd0, 0xd0, 0xd0], [0xda, 0xda, 0xda], [0xe4, 0xe4, 0xe4],
    [0xee, 0xee, 0xee],
];
