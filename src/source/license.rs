//! Best-effort identification of license texts and the warning raised when a
//! Skill's license is missing or unclear.

use std::fmt;

use crate::collection::lock::LicenseFile;
use crate::safety::{Name, RelativePath};

/// GNU licenses by the name in their title, each with its versions. The
/// full texts name one another further down (the GPL-3.0 mentions the AGPL
/// and the LGPL, the MPL-2.0 all of them), so only the name that comes first
/// near the top of the text counts, with the version right after it. Neither
/// `gnu general public license` nor any other name here contains another.
const GNU_LICENSES: &[(&str, &[(&str, &str)])] = &[
    (
        "gnu affero general public license",
        &[("version 3", "AGPL-3.0")],
    ),
    (
        "gnu lesser general public license",
        &[("version 3", "LGPL-3.0"), ("version 2.1", "LGPL-2.1")],
    ),
    (
        "gnu library general public license",
        &[("version 2", "LGPL-2.0")],
    ),
    (
        "gnu general public license",
        &[("version 3", "GPL-3.0"), ("version 2", "GPL-2.0")],
    ),
];
/// Bytes of normalised text, from its start, in which a GNU license name
/// counts as the title or the notice naming the license.
const GNU_NAME_WITHIN: usize = 600;
/// Bytes after a GNU license name in which its version is looked for, long
/// enough for "as published by the Free Software Foundation, either".
const GNU_VERSION_WITHIN: usize = 120;

/// Grant sentence shared by MIT and MIT-0.
const MIT_GRANT: &str =
    "permission is hereby granted, free of charge, to any person obtaining a copy";
/// Grant sentence of ISC, which 0BSD ends without a condition.
const ISC_GRANT: &str =
    "distribute this software for any purpose with or without fee is hereby granted";
/// Opening of every BSD license.
const BSD_GRANT: &str = "redistribution and use in source and binary forms";
/// Clause of BSD-3-Clause and BSD-4-Clause, in either of its usual wordings
/// ("neither the name ... may be used to", "the name ... may not be used
/// to"). A second BSD-3-Clause entry accepts the opening of the first
/// wording alone.
const BSD_ENDORSEMENT: &str = "endorse or promote products derived from this software";
/// Clause of BSD-4-Clause.
const BSD_ADVERTISING: &str =
    "all advertising materials mentioning features or use of this software";

/// Characteristic phrases of other common licenses, checked in order against
/// the text lowercased with whitespace collapsed. Every phrase of an entry
/// must appear and none of its exclusions. Exclusions keep a license from
/// being read as a laxer relative, such as MIT as MIT-0 or BSD-3-Clause as
/// BSD-2-Clause: a text matching no entry exactly stays unidentified, so the
/// unclear-license warning fires rather than a wrong summary being recorded.
const SIGNATURES: &[(&str, &[&str], &[&str])] = &[
    ("Apache-2.0", &["apache license", "version 2.0"], &[]),
    ("MPL-2.0", &["mozilla public license", "2.0"], &[]),
    (
        "MIT",
        &[
            MIT_GRANT,
            "the above copyright notice and this permission notice shall be included in all copies",
        ],
        &[],
    ),
    (
        "MIT-0",
        &[MIT_GRANT, "to whom the software is furnished to do so."],
        &[
            "copyright notice",
            "shall be included",
            "subject to the following conditions",
        ],
    ),
    (
        "ISC",
        &[
            ISC_GRANT,
            "provided that the above copyright notice and this permission notice appear in all copies",
        ],
        &[],
    ),
    (
        "0BSD",
        &["distribute this software for any purpose with or without fee is hereby granted."],
        &["copyright notice"],
    ),
    (
        "BSD-4-Clause",
        &[BSD_GRANT, BSD_ADVERTISING, BSD_ENDORSEMENT],
        &[],
    ),
    (
        "BSD-3-Clause",
        &[BSD_GRANT, BSD_ENDORSEMENT],
        &["advertising materials"],
    ),
    (
        "BSD-3-Clause",
        &[BSD_GRANT, "neither the name of"],
        &["advertising materials"],
    ),
    (
        "BSD-2-Clause",
        &[BSD_GRANT],
        // A third clause, however worded or numbered, makes it another
        // license.
        &["endorse", "neither the name of", "advertising", " 3. "],
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
/// present, otherwise the GNU license named near the top, otherwise the
/// first known license whose characteristic phrases all appear and none of
/// its exclusions. Returns `None` when the license cannot be identified,
/// including a GNU license named without a known version.
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
    if let Some(gnu) = identify_gnu(&normalised) {
        return gnu.map(str::to_owned);
    }
    SIGNATURES
        .iter()
        .find(|(_, phrases, exclusions)| {
            phrases.iter().all(|phrase| normalised.contains(phrase))
                && !exclusions.iter().any(|phrase| normalised.contains(phrase))
        })
        .map(|(id, _, _)| (*id).to_owned())
}

/// The GNU license named first near the top of `normalised`: `None` when no
/// GNU license is named there, `Some(None)` when its version is unknown.
fn identify_gnu(normalised: &str) -> Option<Option<&'static str>> {
    let (start, name, versions) = GNU_LICENSES
        .iter()
        .filter_map(|(name, versions)| {
            let start = normalised.find(name)?;
            (start < GNU_NAME_WITHIN).then_some((start, *name, *versions))
        })
        .min_by_key(|(start, _, _)| *start)?;
    let after = normalised.get(start + name.len()..).unwrap_or_default();
    Some(
        versions
            .iter()
            .filter_map(|(version, id)| {
                let at = after.find(version)?;
                (at < GNU_VERSION_WITHIN).then_some((at, *id))
            })
            .min_by_key(|(at, _)| *at)
            .map(|(_, id)| id),
    )
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
                "Permission is hereby granted, free of charge, to any person obtaining a copy of this software, to deal in the Software without restriction, and to permit persons to whom the Software is furnished to do so.",
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
                "Permission to use, copy, modify, and/or distribute this software for any\npurpose with or without fee is hereby granted, provided that the above\ncopyright notice and this permission notice appear in all copies.",
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
