//! License identification against full license texts. The texts of the
//! copyleft and Creative Commons licenses mention related licenses, so title
//! snippets alone would not catch a misreading.

use skill_curator::source::identify_license;

/// Canonical texts (SPDX license list), each with the identifier expected.
const CANONICAL: &[(&str, &str)] = &[
    ("AGPL-3.0", include_str!("fixtures/licenses/AGPL-3.0.txt")),
    ("GPL-3.0", include_str!("fixtures/licenses/GPL-3.0.txt")),
    ("GPL-2.0", include_str!("fixtures/licenses/GPL-2.0.txt")),
    ("LGPL-3.0", include_str!("fixtures/licenses/LGPL-3.0.txt")),
    ("LGPL-2.1", include_str!("fixtures/licenses/LGPL-2.1.txt")),
    ("LGPL-2.0", include_str!("fixtures/licenses/LGPL-2.0.txt")),
    ("MPL-2.0", include_str!("fixtures/licenses/MPL-2.0.txt")),
    (
        "Apache-2.0",
        include_str!("fixtures/licenses/Apache-2.0.txt"),
    ),
    ("MIT", include_str!("fixtures/licenses/MIT.txt")),
    ("MIT-0", include_str!("fixtures/licenses/MIT-0.txt")),
    ("ISC", include_str!("fixtures/licenses/ISC.txt")),
    ("0BSD", include_str!("fixtures/licenses/0BSD.txt")),
    (
        "BSD-2-Clause",
        include_str!("fixtures/licenses/BSD-2-Clause.txt"),
    ),
    (
        "BSD-3-Clause",
        include_str!("fixtures/licenses/BSD-3-Clause.txt"),
    ),
    (
        "BSD-4-Clause",
        include_str!("fixtures/licenses/BSD-4-Clause.txt"),
    ),
    ("Unlicense", include_str!("fixtures/licenses/Unlicense.txt")),
    ("CC0-1.0", include_str!("fixtures/licenses/CC0-1.0.txt")),
    ("CC-BY-4.0", include_str!("fixtures/licenses/CC-BY-4.0.txt")),
    (
        "CC-BY-SA-4.0",
        include_str!("fixtures/licenses/CC-BY-SA-4.0.txt"),
    ),
];

#[test]
fn identifies_canonical_full_texts() {
    for (expected, text) in CANONICAL {
        assert_eq!(
            identify_license(text.as_bytes()).as_deref(),
            Some(*expected),
            "full text of {expected}"
        );
    }
}

#[test]
fn identifies_gnu_license_notices() {
    let cases = [
        (
            "Copyright (C) 2026 Example Author\n\nThis program is free software: you can redistribute it and/or modify\nit under the terms of the GNU General Public License as published by\nthe Free Software Foundation, either version 3 of the License, or\n(at your option) any later version.",
            "GPL-3.0",
        ),
        (
            "This library is free software; you can redistribute it and/or\nmodify it under the terms of the GNU Lesser General Public\nLicense as published by the Free Software Foundation; either\nversion 2.1 of the License, or (at your option) any later version.",
            "LGPL-2.1",
        ),
        (
            "This program is free software: you can redistribute it and/or modify\nit under the terms of the GNU Affero General Public License as published\nby the Free Software Foundation, either version 3 of the License, or\n(at your option) any later version.",
            "AGPL-3.0",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(
            identify_license(text.as_bytes()).as_deref(),
            Some(expected),
            "{text}"
        );
    }
}

/// Texts close to a known license but not equal to it stay unidentified, so
/// the unclear-license warning fires instead of a wrong summary.
#[test]
fn near_misses_stay_unidentified() {
    let cases = [
        (
            "MIT with \"(including the next paragraph)\"",
            include_str!("fixtures/licenses/variants/MIT-next-paragraph.txt"),
        ),
        (
            "MIT grant with custom conditions",
            include_str!("fixtures/licenses/variants/MIT-custom-conditions.txt"),
        ),
        (
            "Creative Commons BY-NC",
            include_str!("fixtures/licenses/CC-BY-NC-4.0.txt"),
        ),
        (
            "a GNU license without a version",
            "This program is distributed under the GNU General Public License.",
        ),
    ];
    for (label, text) in cases {
        assert_eq!(identify_license(text.as_bytes()), None, "{label}");
    }
}

#[test]
fn identifies_a_bsd_name_clause_worded_differently() {
    assert_eq!(
        identify_license(
            include_str!("fixtures/licenses/variants/BSD-3-Clause-names-may-not.txt").as_bytes()
        )
        .as_deref(),
        Some("BSD-3-Clause")
    );
}
