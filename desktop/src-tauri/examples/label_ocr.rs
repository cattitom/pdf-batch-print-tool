// 临时工具：跑产品标签 OCR，检查「一张图 → 编码 + 名称」的效果。
// 用法：cargo run --release --example label_ocr -- <图片1> <图片2> ...
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：cargo run --release --example label_ocr -- <图片...>");
        std::process::exit(2);
    }
    for path in args {
        match desktop_lib::ocr::parse_label_image(&path) {
            Ok((code, name)) => println!("OK\t{code}\t{name}\t{path}"),
            Err(e) => println!("ERR\t{e}\t{path}"),
        }
    }
}
