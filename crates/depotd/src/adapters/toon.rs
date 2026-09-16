use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Scalar(String),
    Rows(Vec<String>),
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    entries: Vec<(String, Entry)>,
}

impl Document {
    pub fn parse(input: &str) -> Result<Self, ToonError> {
        let mut document = Document::default();
        let mut sections: Vec<(usize, String)> = Vec::new();
        let mut open: Option<Open> = None;

        for (offset, raw) in input.lines().enumerate() {
            let line = offset + 1;
            let text = raw.trim_end();
            if text.trim().is_empty() {
                continue;
            }
            let indent = text.len() - text.trim_start().len();
            let text = text.trim_start();

            if let Some(pending) = open.as_mut()
                && indent > pending.indent
            {
                pending.push(line, text)?;
                if pending.is_full() {
                    let finished = open.take().expect("open entry is present");
                    document.entries.push(finished.into_entry());
                }
                continue;
            }
            if let Some(finished) = open.take() {
                document.entries.push(finished.close(line)?);
            }

            while sections
                .last()
                .is_some_and(|(section_indent, _)| *section_indent >= indent)
            {
                sections.pop();
            }

            match classify(text)? {
                Line::Section { key } => {
                    sections.push((indent, key.to_owned()));
                }
                Line::Scalar { key, value } => {
                    let key = qualify(&sections, key);
                    document
                        .entries
                        .push((key, Entry::Scalar(unquote(value, line)?)));
                }
                Line::Header { key, shape } => {
                    let key = qualify(&sections, key);
                    let header = Open::new(key, indent, shape);
                    if header.is_full() {
                        document.entries.push(header.into_entry());
                    } else {
                        open = Some(header);
                    }
                }
            }
        }

        if let Some(finished) = open {
            let line = input.lines().count();
            document.entries.push(finished.close(line)?);
        }

        Ok(document)
    }

    pub fn scalar(&self, key: &str) -> Option<&str> {
        self.entries.iter().find_map(|(name, entry)| match entry {
            Entry::Scalar(value) if leaf(name) == key => Some(value.as_str()),
            _ => None,
        })
    }

    pub fn rows(&self, key: &str) -> Option<&[String]> {
        self.entries.iter().find_map(|(name, entry)| match entry {
            Entry::Rows(rows) if leaf(name) == key => Some(rows.as_slice()),
            _ => None,
        })
    }

    pub fn table(&self, key: &str) -> Option<Table<'_>> {
        self.entries.iter().find_map(|(name, entry)| match entry {
            Entry::Table { columns, rows } if leaf(name) == key => Some(Table {
                columns: columns.as_slice(),
                rows: rows.as_slice(),
            }),
            _ => None,
        })
    }

    pub fn keys(&self) -> Vec<&str> {
        self.entries.iter().map(|(name, _)| leaf(name)).collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Table<'a> {
    pub columns: &'a [String],
    pub rows: &'a [Vec<String>],
}

impl Table<'_> {
    pub fn column(&self, name: &str) -> Result<usize, ToonError> {
        self.columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| ToonError::MissingColumn {
                key: name.to_owned(),
                columns: self.columns.to_vec(),
            })
    }
}

struct Open {
    key: String,
    indent: usize,
    expected: usize,
    pending: Pending,
}

impl Open {
    fn new(key: String, indent: usize, shape: Shape) -> Self {
        let pending = match shape.columns {
            Some(columns) => Pending::Table {
                columns,
                rows: Vec::new(),
            },
            None => Pending::Rows(Vec::new()),
        };
        Self {
            key,
            indent,
            expected: shape.count,
            pending,
        }
    }

    fn is_full(&self) -> bool {
        self.rows() == self.expected
    }

    fn rows(&self) -> usize {
        match &self.pending {
            Pending::Rows(rows) => rows.len(),
            Pending::Table { rows, .. } => rows.len(),
        }
    }

    fn push(&mut self, line: usize, text: &str) -> Result<(), ToonError> {
        match &mut self.pending {
            Pending::Rows(rows) => rows.push(unquote(text, line)?),
            Pending::Table { columns, rows } => {
                let fields = split_quoted(text, line)?
                    .into_iter()
                    .map(|field| unquote(field.trim(), line))
                    .collect::<Result<Vec<_>, _>>()?;
                if fields.len() != columns.len() {
                    return Err(ToonError::ColumnCount {
                        key: self.key.clone(),
                        expected: columns.len(),
                        found: fields.len(),
                        line,
                    });
                }
                rows.push(fields);
            }
        }
        Ok(())
    }

    fn close(self, line: usize) -> Result<(String, Entry), ToonError> {
        let found = self.rows();
        if found != self.expected {
            return Err(ToonError::RowCount {
                key: self.key,
                expected: self.expected,
                found,
                line,
            });
        }
        Ok(self.into_entry())
    }

    fn into_entry(self) -> (String, Entry) {
        let entry = match self.pending {
            Pending::Rows(rows) => Entry::Rows(rows),
            Pending::Table { columns, rows } => Entry::Table { columns, rows },
        };
        (self.key, entry)
    }
}

enum Pending {
    Rows(Vec<String>),
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

struct Shape {
    count: usize,
    columns: Option<Vec<String>>,
}

enum Line<'a> {
    Section { key: &'a str },
    Scalar { key: &'a str, value: &'a str },
    Header { key: &'a str, shape: Shape },
}

fn classify(text: &str) -> Result<Line<'_>, ToonError> {
    if let Some((key, value)) = text.split_once(": ") {
        return Ok(Line::Scalar { key, value });
    }
    let Some(rest) = text.strip_suffix(':') else {
        return Err(ToonError::Unreadable(text.to_owned()));
    };
    let Some((key, header)) = rest.split_once('[') else {
        return Ok(Line::Section { key: rest });
    };
    let Some((count, columns)) = header.split_once(']') else {
        return Err(ToonError::Unreadable(text.to_owned()));
    };
    let count = count
        .parse::<usize>()
        .map_err(|_| ToonError::Unreadable(text.to_owned()))?;
    let columns = match columns {
        "" => None,
        columns => Some(parse_columns(columns)?),
    };
    Ok(Line::Header {
        key,
        shape: Shape { count, columns },
    })
}

fn parse_columns(text: &str) -> Result<Vec<String>, ToonError> {
    let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) else {
        return Err(ToonError::Unreadable(text.to_owned()));
    };
    Ok(inner
        .split(',')
        .map(|column| column.trim().to_owned())
        .collect())
}

fn leaf(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

fn qualify(sections: &[(usize, String)], key: &str) -> String {
    if sections.is_empty() {
        return key.to_owned();
    }
    let prefix: Vec<&str> = sections.iter().map(|(_, name)| name.as_str()).collect();
    format!("{}.{key}", prefix.join("."))
}

fn split_quoted(text: &str, line: usize) -> Result<Vec<String>, ToonError> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => {
                current.push(character);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                current.push(character);
            }
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    if quoted {
        return Err(ToonError::UnterminatedQuote { line });
    }
    fields.push(current);
    Ok(fields)
}

fn unquote(text: &str, line: usize) -> Result<String, ToonError> {
    if !text.starts_with('"') {
        return Ok(text.to_owned());
    }
    let mut value = String::new();
    let mut characters = text[1..].chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                if characters.next().is_some() {
                    return Err(ToonError::TrailingCharacters {
                        line,
                        text: text.to_owned(),
                    });
                }
                return Ok(value);
            }
            '\\' => match characters.next() {
                Some('\\') => value.push('\\'),
                Some('"') => value.push('"'),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                Some(other) => {
                    return Err(ToonError::UnknownEscape {
                        line,
                        escape: format!("\\{other}"),
                    });
                }
                None => return Err(ToonError::UnterminatedQuote { line }),
            },
            other => value.push(other),
        }
    }
    Err(ToonError::UnterminatedQuote { line })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonError {
    Unreadable(String),
    UnterminatedQuote {
        line: usize,
    },
    TrailingCharacters {
        line: usize,
        text: String,
    },
    UnknownEscape {
        line: usize,
        escape: String,
    },
    RowCount {
        key: String,
        expected: usize,
        found: usize,
        line: usize,
    },
    ColumnCount {
        key: String,
        expected: usize,
        found: usize,
        line: usize,
    },
    MissingColumn {
        key: String,
        columns: Vec<String>,
    },
}

impl fmt::Display for ToonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToonError::Unreadable(text) => write!(f, "unreadable line {text:?}"),
            ToonError::UnterminatedQuote { line } => {
                write!(f, "line {line} opens a quote it never closes")
            }
            ToonError::TrailingCharacters { line, text } => {
                write!(f, "line {line} has text after the closing quote: {text:?}")
            }
            ToonError::UnknownEscape { line, escape } => {
                write!(f, "line {line} uses the unknown escape {escape:?}")
            }
            ToonError::RowCount {
                key,
                expected,
                found,
                line,
            } => write!(
                f,
                "{key} announces {expected} rows but line {line} ends it with {found}"
            ),
            ToonError::ColumnCount {
                key,
                expected,
                found,
                line,
            } => write!(
                f,
                "{key} row on line {line} has {found} fields where {expected} were announced"
            ),
            ToonError::MissingColumn { key, columns } => {
                write!(f, "no {key} column in the announced columns {columns:?}")
            }
        }
    }
}

impl std::error::Error for ToonError {}
