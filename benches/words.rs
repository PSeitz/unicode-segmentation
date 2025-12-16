//! Compares the performance of `UnicodeSegmentation::unicode_words` with stdlib's UTF-8
//! scalar-based `std::str::split_whitespace`.
//!
//! It is expected that `std::str::split_whitespace` is faster than
//! `UnicodeSegmentation::unicode_words` since it does not consider the complexity of grapheme
//! clusters. The question in this benchmark is how much slower full unicode handling is.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use std::{fs, time::Duration};
use unicode_segmentation::UnicodeSegmentation;

const FILES: &[&str] = &[
    "log",
    "arabic",
    "english",
    "hindi",
    "japanese",
    "korean",
    "mandarin",
    "russian",
    "source_code",
];

#[inline(always)]
fn grapheme(text: &str) {
    for w in text.unicode_words() {
        black_box(w);
    }
}

#[inline(always)]
fn split_whitespace(text: &str) {
    for w in text.split_whitespace() {
        black_box(w);
    }
}

fn bench_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("words");
    group.warm_up_time(Duration::from_millis(200));

    for file in FILES {
        let input = fs::read_to_string(format!("benches/texts/{file}.txt")).unwrap();
        group.throughput(criterion::Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("unicode_words", file),
            &input,
            |b, content| b.iter(|| grapheme(content)),
        );
    }

    for file in FILES {
        let input = fs::read_to_string(format!("benches/texts/{file}.txt")).unwrap();
        group.throughput(criterion::Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("split_whitespace", file),
            &input,
            |b, content| b.iter(|| split_whitespace(content)),
        );
    }
}

criterion_group!(benches, bench_all);
criterion_main!(benches);
