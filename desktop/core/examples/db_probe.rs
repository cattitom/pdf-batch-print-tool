use pdfimpose_core::{Db, rusqlite};
fn main() {
    let path = std::env::args().nth(1).expect("isolated database path required");
    let conn = rusqlite::Connection::open(&path).expect("open");
    println!("readonly={:?}", conn.is_readonly("main"));
    println!("WAL={:?}", conn.pragma_update(None, "journal_mode", "WAL"));
    drop(conn);
    match Db::open(&path) {
        Ok(_) => println!("Db::open: OK"),
        Err(e) => { eprintln!("Db::open: {e:?}"); std::process::exit(1); }
    }
}
