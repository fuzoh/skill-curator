//! Best-effort identification of license texts and the warning raised when a
//! Skill's license is missing or unclear.

use std::fmt;

use crate::collection::lock::LicenseFile;
use crate::safety::{Name, RelativePath};

/// Characteristic phrases of common licenses, checked in order against the
/// text lowercased with whitespace collapsed. Every phrase of an entry must
/// appear and none of its exclusions. More specific licenses come first:
/// the LGPL text mentions the GPL, and MIT-0 is MIT without its condition.
const SIGNATURES: &[(&str, &[&str], &[&str])] = &[
    (
        "AGPL-3.0",
        &["gnu affero general public license", "version 3"],
        &[],
    ),
    (
        "LGPL-3.0",
        &["gnu lesser general public license", "version 3"],
        &[],
    ),
    (
        "LGPL-2.1",
        &["gnu lesser general public license", "version 2.1"],
        &[],
    ),
    ("GPL-3.0", &["gnu general public license", "version 3"], &[]),
    ("GPL-2.0", &["gnu general public license", "version 2"], &[]),
    ("Apache-2.0", &["apache license", "version 2.0"], &[]),
    ("MPL-2.0", &["mozilla public license", "2.0"], &[]),
    (
        "MIT",
        &[
            "permission is hereby granted, free of charge, to any person obtaining a copy",
            "the above copyright notice and this permission notice shall be included",
        ],
        &[],
    ),
    (
        "MIT-0",
        &["permission is hereby granted, free of charge, to any person obtaining a copy"],
        &[],
    ),
    (
        "ISC",
        &["permission to use, copy, modify, and/or distribute this software for any purpose"],
        &[],
    ),
    (
        "BSD-3-Clause",
        &[
            "redistribution and use in source and binary forms",
            "neither the name of",
        ],
        &[],
    ),
    (
        "BSD-2-Clause",
        &["redistribution and use in source and binary forms"],
        &["neither the name of"],
    ),
    (
        "Unlicense",
        &["this is free and unencumbered software released into the public domain"],
        &[],
    ),
    ("CC0-1.0", &["cc0 1.0 universal"], &[]),
    (
        "CC-BY-SA-4.0",
        &["attribution-sharealike 4.0 international"],
        &[],
    ),
    ("CC-BY-4.0", &["attribution 4.0 international"], &[]),
];

/// Identifies the license in `text`: an `SPDX-License-Identifier:` tag when
/// present, otherwise the first known license whose characteristic phrases
/// all appear. Returns `None` when the license cannot be identified.
#[must_use]
pub fn identify(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    if let Some(tag) = text.lines().find_map(|line| {
        let (_, value) = line.split_once("SPDX-License-Identifier:")?;
        let value = value.trim().trim_end_matches("*/").trim();
        (!value.is_empty()).then(|| value.to_owned())
    }) {
        return Some(tag);
    }
    let normalised = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    SIGNATURES
        .iter()
        .find(|(_, phrases, exclusions)| {
            phrases.iter().all(|phrase| normalised.contains(phrase))
                && !exclusions.iter().any(|phrase| normalised.contains(phrase))
        })
        .map(|(id, _, _)| (*id).to_owned())
}

/// A Skill whose license could not be established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseWarning {
    /// Source the Skill comes from.
    pub source_name: Name,
    /// Skill directory inside the Source.
    pub upstream_path: RelativePath,
    /// What is wrong.
    pub kind: LicenseWarningKind,
}

/// Why a Skill's license could not be established.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LicenseWarningKind {
    /// No license file was found.
    Missing,
    /// License files were found, but these could not be identified.
    Unclear {
        /// Paths of the unidentified files, relative to the Source root.
        paths: Vec<RelativePath>,
    },
}

impl LicenseWarning {
    /// The warning for a Skill with license files `licenses`, or `None` when
    /// at least one was found and every one was identified.
    #[must_use]
    pub fn check(
        source_name: &Name,
        upstream_path: &RelativePath,
        licenses: &[LicenseFile],
    ) -> Option<Self> {
        let kind = if licenses.is_empty() {
            LicenseWarningKind::Missing
        } else {
            let paths = licenses
                .iter()
                .filter(|license| license.summary.is_none())
                .map(|license| license.path.clone())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                return None;
            }
            LicenseWarningKind::Unclear { paths }
        };
        Some(Self {
            source_name: source_name.clone(),
            upstream_path: upstream_path.clone(),
            kind,
        })
    }
}

impl fmt::Display for LicenseWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let skill = format!(
            "Skill `{}` of Source `{}`",
            self.upstream_path, self.source_name
        );
        match &self.kind {
            LicenseWarningKind::Missing => write!(
                f,
                "no license file found for {skill} (looked for LICENSE*, LICENCE*, COPYING*, and UNLICENSE* at the Source root, in each directory leading to the Skill, and in the Skill directory); check the Source's terms before redistributing it"
            ),
            LicenseWarningKind::Unclear { paths } => {
                let paths = paths
                    .iter()
                    .map(|path| format!("`{path}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "unclear license for {skill}: could not identify {paths}; read the file and check the Source's terms before redistributing it"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIT: &str = "MIT License\n\nCopyright (c) 2026 Someone\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction.\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n";

    #[test]
    fn identifies_common_licenses_from_their_text() {
        let cases = [
            (MIT, "MIT"),
            (
                "Permission is hereby granted, free of charge, to any person obtaining a copy of this software, to deal in the Software without restriction.",
                "MIT-0",
            ),
            (
                "                                 Apache License\n                           Version 2.0, January 2004",
                "Apache-2.0",
            ),
            (
                "GNU GENERAL PUBLIC LICENSE\n Version 3, 29 June 2007",
                "GPL-3.0",
            ),
            (
                "GNU GENERAL PUBLIC LICENSE\n Version 2, June 1991",
                "GPL-2.0",
            ),
            (
                "GNU LESSER GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007\n... the GNU General Public License ...",
                "LGPL-3.0",
            ),
            (
                "GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 3, 19 November 2007",
                "AGPL-3.0",
            ),
            ("Mozilla Public License Version 2.0", "MPL-2.0"),
            (
                "Permission to use, copy, modify, and/or distribute this software for any\npurpose with or without fee is hereby granted",
                "ISC",
            ),
            (
                "Redistribution and use in source and binary forms, with or without modification... Neither the name of the copyright holder",
                "BSD-3-Clause",
            ),
            (
                "Redistribution and use in source and binary forms, with or without modification",
                "BSD-2-Clause",
            ),
            (
                "This is free and unencumbered software released into the public domain.",
                "Unlicense",
            ),
            (
                "Creative Commons Legal Code\n\nCC0 1.0 Universal",
                "CC0-1.0",
            ),
            ("Attribution-ShareAlike 4.0 International", "CC-BY-SA-4.0"),
            ("Attribution 4.0 International", "CC-BY-4.0"),
        ];
        for (text, expected) in cases {
            assert_eq!(
                identify(text.as_bytes()).as_deref(),
                Some(expected),
                "{text}"
            );
        }
    }

    #[test]
    fn prefers_an_spdx_tag() {
        assert_eq!(
            identify(
                b"/* SPDX-License-Identifier: MIT OR Apache-2.0 */\nApache License Version 2.0"
            )
            .as_deref(),
            Some("MIT OR Apache-2.0")
        );
        assert_eq!(identify(b"SPDX-License-Identifier:\n").as_deref(), None);
    }

    #[test]
    fn unknown_texts_are_unidentified() {
        for text in [
            "",
            "All rights reserved.",
            "Do what you want, but ask first.",
        ] {
            assert_eq!(identify(text.as_bytes()), None, "{text}");
        }
    }

    fn license(path: &str, summary: Option<&str>) -> LicenseFile {
        LicenseFile {
            path: RelativePath::new(path).unwrap(),
            summary: summary.map(str::to_owned),
        }
    }

    #[test]
    fn warns_when_no_license_is_found_or_one_is_unclear() {
        let source = Name::new("pstack").unwrap();
        let skill = RelativePath::new("pstack/critique").unwrap();

        let missing = LicenseWarning::check(&source, &skill, &[]).unwrap();
        assert_eq!(missing.kind, LicenseWarningKind::Missing);
        assert!(
            missing.to_string().starts_with(
                "no license file found for Skill `pstack/critique` of Source `pstack`"
            ),
            "{missing}"
        );

        let licenses = [
            license("LICENSE", Some("MIT")),
            license("pstack/LICENSE", None),
        ];
        let unclear = LicenseWarning::check(&source, &skill, &licenses).unwrap();
        assert_eq!(
            unclear.kind,
            LicenseWarningKind::Unclear {
                paths: vec![RelativePath::new("pstack/LICENSE").unwrap()]
            }
        );
        assert!(
            unclear
                .to_string()
                .contains("could not identify `pstack/LICENSE`"),
            "{unclear}"
        );

        assert_eq!(
            LicenseWarning::check(&source, &skill, &[license("LICENSE", Some("MIT"))]),
            None
        );
    }
}
