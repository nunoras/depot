use depotd::adapters::toon::{Document, ToonError};

#[test]
fn reads_fields_under_sections_and_string_rows() {
    let document = Document::parse(
        "skill:\n  action: install\n  harnesses: 2\nskills[2]:\n  prompt\n  verify\n",
    )
    .expect("the document is read");

    assert_eq!(document.scalar("action"), Some("install"));
    assert_eq!(document.scalar("harnesses"), Some("2"));
    assert_eq!(
        document.rows("skills"),
        Some(["prompt".to_owned(), "verify".to_owned()].as_slice())
    );
    assert_eq!(document.keys(), vec!["action", "harnesses", "skills"]);
}

#[test]
fn reads_quoted_scalars_and_escapes() {
    let document = Document::parse(
        "session: 4f2a91\nmessage: \"the redirect points at /login\"\nempty: \"\"\npath: \"a\\\\b\"\n",
    )
    .expect("the document is read");

    assert_eq!(document.scalar("session"), Some("4f2a91"));
    assert_eq!(
        document.scalar("message"),
        Some("the redirect points at /login")
    );
    assert_eq!(document.scalar("empty"), Some(""));
    assert_eq!(document.scalar("path"), Some("a\\b"));
}

#[test]
fn reads_tabular_rows() {
    let document = Document::parse(
        "sessions[2]{id,state,harness}:\n  4f2a91,finished,claude\n  9b01cc,running,pi\n",
    )
    .expect("the table is read");

    let table = document.table("sessions").expect("the table is present");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.column("state").expect("the column is present"), 1);
    assert_eq!(table.rows[1], vec!["9b01cc", "running", "pi"]);
    assert_eq!(document.scalar("state"), None);
}

#[test]
fn keeps_a_quoted_field_whole() {
    let document = Document::parse(
        "sessions[1]{id,note}:\n  4f2a91,\"finished, with notes, and a \\\"quote\\\"\"\n",
    )
    .expect("the table is read");

    let table = document.table("sessions").expect("the table is present");
    assert_eq!(table.rows[0][1], "finished, with notes, and a \"quote\"");
}

#[test]
fn reads_an_empty_array() {
    let document = Document::parse("sessions[0]{id,state}:\n").expect("the table is read");
    let table = document.table("sessions").expect("the table is present");
    assert!(table.rows.is_empty());
    assert_eq!(table.columns, ["id", "state"]);
}

#[test]
fn refuses_a_line_it_cannot_read() {
    let error = Document::parse("this is not toon\n").expect_err("the line is refused");
    assert!(matches!(error, ToonError::Unreadable(_)), "{error}");
    assert!(error.to_string().contains("not toon"), "{error}");
}

#[test]
fn refuses_rows_that_disagree_with_the_header() {
    let error = Document::parse("sessions[2]{id,state}:\n  4f2a91,finished\n")
        .expect_err("the rows are refused");
    assert!(error.to_string().contains("announces 2 rows"), "{error}");

    let error = Document::parse("sessions[2]{id,state}:\n  4f2a91,finished\nstatus: ok\n")
        .expect_err("the rows are refused");
    assert!(error.to_string().contains("ends it with 1"), "{error}");
}

#[test]
fn refuses_a_row_with_the_wrong_number_of_fields() {
    let error =
        Document::parse("sessions[1]{id,state}:\n  4f2a91\n").expect_err("the row is refused");
    assert!(error.to_string().contains("1 fields where 2"), "{error}");
}

#[test]
fn refuses_an_unterminated_quote() {
    let error = Document::parse("message: \"open\n").expect_err("the quote is refused");
    assert!(error.to_string().contains("never closes"), "{error}");
}

#[test]
fn refuses_text_after_a_closing_quote() {
    let error = Document::parse("message: \"closed\" and more\n").expect_err("the line is refused");
    assert!(
        error.to_string().contains("after the closing quote"),
        "{error}"
    );
}

#[test]
fn refuses_an_unknown_escape() {
    let error = Document::parse("message: \"a\\qb\"\n").expect_err("the escape is refused");
    assert!(error.to_string().contains("\\\\q"), "{error}");
}

#[test]
fn refuses_a_missing_column() {
    let document = Document::parse("sessions[1]{id}:\n  4f2a91\n").expect("the table is read");
    let table = document.table("sessions").expect("the table is present");
    let error = table.column("state").expect_err("the column is refused");
    assert!(error.to_string().contains("no state column"), "{error}");
}
