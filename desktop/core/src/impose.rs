//! 拼版合并：多 PDF 按目标页数凑页/截取复制，支持旋转、
//! 裁剪与文件间分隔页。
//!
//! 与 Streamlit 版 `build_merged_pdf` 的行为对齐：
//! - 第一个文件前不插分隔页，其后每个文件前插 `sep_pages` 张
//! - 分隔页尺寸跟随其后文件第一页应用裁剪 + 旋转后的显示尺寸
//! - 同一内容页被复制多份时共享内容流/资源（体积不膨胀的关键）
//!
//! 本模块只做编排：分隔页见 [`crate::sep`]，页码见 [`crate::numbers`]。
//! 输出直接写进 `Write`（见 [`impose_to`]），源文档逐文件解析、用完即释放。

use crate::error::ImposeError;
use crate::crop::{auto_crop_rect, crop_rect, pdfium, CropSpec};
use crate::{numbers, sep};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::time::{Duration, Instant};

pub const MM_TO_PT: f64 = 72.0 / 25.4;

/// 输入来源。
///
/// `Path` 是常规用法：core 在轮到该文件时才读取，读完即释放，峰值内存只跟
/// 单个文件有关；`Bytes` 供测试或调用方已经在内存里的 PDF 使用。
#[derive(Debug)]
pub enum Source {
    Path(std::path::PathBuf),
    Bytes(Vec<u8>),
}

impl Source {
    /// 取字节：路径来源此时才读盘（`Cow` 避免 Bytes 分支再复制一份）。
    fn load(&self) -> ImposeResult<std::borrow::Cow<'_, [u8]>> {
        Ok(match self {
            Source::Bytes(bytes) => std::borrow::Cow::Borrowed(bytes.as_slice()),
            Source::Path(path) => std::borrow::Cow::Owned(
                std::fs::read(path)
                    .map_err(|e| ImposeError::io(path.display().to_string(), e))?,
            ),
        })
    }
}

/// 单个输入文件及其拼版参数。
#[derive(Debug)]
pub struct FileInput {
    pub name: String,
    pub source: Source,
    /// 目标页数：>原页数时循环复制凑页，<原页数时截取前 N 页
    pub target_pages: u32,
    /// 旋转角度（90 / 180 / 270，其他值忽略）
    pub rotate: i64,
    pub crop: Option<CropSpec>,
}

impl FileInput {
    /// 用内存中的字节构造（测试、或调用方已生成好的 PDF）。
    pub fn from_bytes(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            source: Source::Bytes(bytes),
            target_pages: 1,
            rotate: 0,
            crop: None,
        }
    }

    /// 用路径构造：真正读取发生在拼版过程中，读完即释放。
    pub fn from_path(name: impl Into<String>, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            name: name.into(),
            source: Source::Path(path.into()),
            target_pages: 1,
            rotate: 0,
            crop: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImposeOptions {
    /// 每个文件（第一个除外）前插入的分隔页数量
    pub sep_pages: u32,
    /// 分隔页文字，默认「这是分隔线」
    pub sep_text: Option<String>,
    /// 在每个文件的内容页右下角盖页码 1,2,3…（每个文件重新从 1 开始，分隔页不编号）
    pub page_numbers: bool,
    /// 每页连续重复 N 份（1 = 不重复）。
    ///
    /// 与「页数」的分工：页数是这份文件出几页（超出就整份轮转凑页），
    /// copies 是把**每一页**再印几份。3 页 × 2 份 = 1,1,2,2,3,3（箱子标签要一箱两份）。
    pub copies: u32,
}

impl Default for ImposeOptions {
    fn default() -> Self {
        Self {
            sep_pages: 0,
            sep_text: None,
            page_numbers: false,
            copies: 1,
        }
    }
}

pub use crate::error::ImposeResult;

/// 拼版结果统计（不需要再回头解析输出文件）。
#[derive(Debug, Clone, Copy)]
pub struct ImposeStats {
    pub pages: usize,
    pub elapsed: Duration,
}

/// 执行拼版，返回输出 PDF 字节（测试与内存调用方用）。
///
/// `progress(done, total)` 按页数回调，用于进度条；每 200 页回调一次
///（与 Streamlit 版一致，避免高频刷新拖慢合成）。
pub fn impose(
    inputs: &[FileInput],
    opts: &ImposeOptions,
    progress: &dyn Fn(usize, usize),
) -> ImposeResult<Vec<u8>> {
    let mut out = Vec::new();
    impose_to(inputs, opts, progress, &mut out)?;
    Ok(out)
}

/// 执行拼版并把 PDF 流式写入 `out`（不把整份输出留在内存里）。
pub fn impose_to<W: Write>(
    inputs: &[FileInput],
    opts: &ImposeOptions,
    progress: &dyn Fn(usize, usize),
    out: &mut W,
) -> ImposeResult<ImposeStats> {
    let t0 = Instant::now();

    // 进度总量只取决于请求页数，无需先解析一遍源文件
    let copies = opts.copies.max(1) as usize;
    let mut total: usize = inputs
        .iter()
        .map(|i| (i.target_pages as usize).max(1) * copies)
        .sum();
    if inputs.len() > 1 && opts.sep_pages > 0 {
        total += opts.sep_pages as usize * (inputs.len() - 1);
    }
    let total = total.max(1);
    let mut done = 0usize;

    let mut dst = new_document();
    let pages_id = dst.add_object(pages_node());
    let catalog_id = dst.add_object(catalog_node(pages_id));
    dst.trailer.set("Root", Object::Reference(catalog_id));

    let mut kids: Vec<Object> = Vec::new();
    let mut numbered: Vec<numbers::NumberedPage> = Vec::new();
    let sep_text = opts.sep_text.clone().unwrap_or_else(|| "这是分隔线".into());

    for (pos, input) in inputs.iter().enumerate() {
        // 源文档只在本文件处理期间存活，下一轮迭代即释放（大文件峰值的关键）
        let bytes = input.source.load()?;
        let src = Document::load_mem(&bytes)?;
        let page_ids: Vec<ObjectId> = src.page_iter().collect();
        if page_ids.is_empty() {
            continue;
        }

        // 第一页的裁剪矩形（决定分隔页尺寸 + 各页裁剪）
        let first_dict = resolved_page(&src, page_ids[0])?;
        let first_size = source_geometry(&first_dict).0;
        let crop_rect_first: Option<(f64, f64, f64, f64)> = match &input.crop {
            Some(CropSpec::Auto { margin_mm }) => {
                auto_crop_rect(&bytes, *margin_mm, 96.0)?
            }
            Some(spec) => Some(crop_rect(spec, first_size)?),
            None => None,
        };

        // 分隔页：尺寸跟随裁剪 + 旋转后的显示尺寸；同尺寸同文字全局只生成一次
        if pos > 0 && opts.sep_pages > 0 {
            let (sw, sh) = display_size(first_size, crop_rect_first, input.rotate);
            let sep_bytes = sep::separator_pdf(sw, sh, &sep_text)?;
            let sep_doc = Document::load_mem(sep_bytes.as_slice())?;
            let sep_page_ids: Vec<ObjectId> = sep_doc.page_iter().collect();
            if let Some(&sep_pid) = sep_page_ids.first() {
                let mut sep_map: HashMap<ObjectId, ObjectId> = HashMap::new();
                for _ in 0..opts.sep_pages {
                    // 每张分隔页都要新建页面字典（Kids 不允许重复引用同一页对象）
                    let nid = copy_page_shallow(&mut dst, &sep_doc, sep_pid, &mut sep_map)?;
                    kids.push(Object::Reference(nid));
                    numbered.push(numbers::NumberedPage {
                        id: nid,
                        width: sw,
                        height: sh,
                        label: None,
                    });
                    done += 1;
                    if done % 200 == 0 {
                        progress(done, total);
                    }
                }
            }
        }

        // 内容页（页码按文件重新从 1 开始计数）
        let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
        let need = (input.target_pages as usize).max(1);
        for k in 0..(need * copies) {
            // 每页先连印 copies 份（1,1,2,2,3,3），再按整个文档轮转凑页
            let pid = page_ids[(k / copies) % page_ids.len()];
            let (nid, w, h) =
                add_content_page(&mut dst, &src, pid, &mut map, input, crop_rect_first)?;
            kids.push(Object::Reference(nid));
            numbered.push(numbers::NumberedPage {
                id: nid,
                width: w,
                height: h,
                label: Some((k + 1).to_string()),
            });
            done += 1;
            if done % 200 == 0 {
                progress(done, total);
            }
        }
    }

    // 页码直接盖在内存文档上（不再序列化→pdfium→再保存）
    if opts.page_numbers {
        numbers::add_page_numbers(&mut dst, &numbered)?;
    }

    if let Object::Dictionary(d) = dst.get_object_mut(pages_id)? {
        d.set("Kids", Object::Array(kids.clone()));
        d.set("Count", kids.len() as i64);
    }

    // 顺序写出，全程不需要一份完整的输出字节
    dst.save_to(out)?;
    progress(total, total);
    let stats = ImposeStats { pages: kids.len(), elapsed: t0.elapsed() };
    log::info!("拼版完成：{} 页，耗时 {:?}", stats.pages, stats.elapsed);
    Ok(stats)
}

// ---------- 文档骨架 ----------

fn new_document() -> Document {
    Document::with_version("1.4")
}

fn pages_node() -> Object {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Pages".to_vec()));
    d.set("Kids", Object::Array(vec![]));
    d.set("Count", Object::Integer(0));
    Object::Dictionary(d)
}

fn catalog_node(pages_id: ObjectId) -> Object {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Catalog".to_vec()));
    d.set("Pages", Object::Reference(pages_id));
    Object::Dictionary(d)
}

// ---------- 页面处理 ----------

fn page_dictionary<'a>(doc: &'a Document, page_id: ObjectId) -> ImposeResult<&'a Dictionary> {
    match doc.get_object(page_id)? {
        Object::Dictionary(d) => Ok(d),
        _ => Err(ImposeError::pdf("页面不是字典对象")),
    }
}

// Materialize inheritable page attributes before detaching from the source tree.
fn resolved_page(src: &Document, id: ObjectId) -> ImposeResult<Dictionary> {
    let mut page = page_dictionary(src, id)?.clone();
    let mut current = id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return Err(ImposeError::pdf("页面父节点存在循环引用"));
        }
        let node = page_dictionary(src, current)?;
        for key in [b"MediaBox".as_slice(), b"CropBox", b"Resources", b"Rotate"] {
            if page.get(key).is_err() {
                if let Ok(value) = node.get(key) { page.set(key.to_vec(), value.clone()); }
            }
        }
        match node.get(b"Parent") {
            Ok(Object::Reference(parent)) => current = *parent,
            Ok(_) => return Err(ImposeError::pdf("页面父节点不是间接引用")),
            Err(_) => break,
        }
    }
    for key in [b"MediaBox".as_slice(), b"CropBox", b"Rotate"] {
        if let Ok(value) = page.get(key).cloned() {
            let (_, resolved) = src.dereference(&value)?;
            page.set(key.to_vec(), resolved.clone());
        }
    }
    page.remove(b"Parent");
    Ok(page)
}

// Transform original PDF coordinates to the visible, unrotated-origin display space.
fn source_geometry(page: &Dictionary) -> ((f64, f64), [f64; 6]) {
    let (x0, y0, x1, y1) = dict_box(page, b"CropBox").unwrap_or_else(|| mediabox_of(page));
    let (w, h) = ((x1-x0).abs(), (y1-y0).abs());
    let rot = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0).rem_euclid(360);
    match rot {
        90 => ((h,w), [0.,-1.,1.,0.,-y0,w+x0]),
        180 => ((w,h), [-1.,0.,0.,-1.,w+x0,h+y0]),
        270 => ((h,w), [0.,1.,-1.,0.,h+y0,-x0]),
        _ => ((w,h), [1.,0.,0.,1.,-x0,-y0]),
    }
}

fn dict_box(dict: &Dictionary, key: &[u8]) -> Option<(f64, f64, f64, f64)> {
    match dict.get(key).ok()? {
        Object::Array(items) => {
            let nums: Vec<f64> = items
                .iter()
                .map(|o| match o {
                    Object::Integer(i) => *i as f64,
                    Object::Real(f) => *f as f64,
                    _ => 0.0,
                })
                .collect();
            if nums.len() == 4 {
                Some((nums[0], nums[1], nums[2], nums[3]))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn mediabox_of(dict: &Dictionary) -> (f64, f64, f64, f64) {
    dict_box(dict, b"MediaBox").unwrap_or((0.0, 0.0, 612.0, 792.0))
}

/// 应用裁剪矩形与旋转后的显示尺寸（分隔页尺寸要用它）。
fn display_size(
    original: (f64, f64),
    crop: Option<(f64, f64, f64, f64)>,
    rotate: i64,
) -> (f64, f64) {
    let (mut w, mut h) = match crop {
        Some((x0, y0, x1, y1)) => ((x1 - x0).abs(), (y1 - y0).abs()),
        None => original,
    };
    let rot = rotate.rem_euclid(360);
    if rot == 90 || rot == 270 {
        std::mem::swap(&mut w, &mut h);
    }
    (w, h)
}

/// 深拷贝 src 里 val 可达的所有间接对象到 dst（带去重映射）。
///
/// 遇到指向不存在对象的引用时返回错误而不是 panic：损坏的 PDF 不应该让整个
/// 进程退出（release 是 `panic = "abort"`，崩溃连提示都没有）。
fn copy_value(
    dst: &mut Document,
    src: &Document,
    val: &Object,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> ImposeResult<Object> {
    Ok(match val {
        Object::Reference(id) => Object::Reference(copy_object(dst, src, *id, map)?),
        Object::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(copy_value(dst, src, v, map)?);
            }
            Object::Array(out)
        }
        Object::Dictionary(dict) => {
            let mut nd = Dictionary::new();
            for (k, v) in dict.iter() {
                nd.set(k.clone(), copy_value(dst, src, v, map)?);
            }
            Object::Dictionary(nd)
        }
        Object::Stream(stream) => {
            let mut nd = Dictionary::new();
            for (k, v) in stream.dict.iter() {
                nd.set(k.clone(), copy_value(dst, src, v, map)?);
            }
            Object::Stream(Stream::new(nd, stream.content.clone()))
        }
        other => other.clone(),
    })
}

fn copy_object(
    dst: &mut Document,
    src: &Document,
    id: ObjectId,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> ImposeResult<ObjectId> {
    if let Some(&nid) = map.get(&id) {
        return Ok(nid);
    }
    let obj = src
        .get_object(id)
        .map_err(|_| ImposeError::pdf(format!("结构损坏：引用了不存在的对象 {} {}", id.0, id.1)))?
        .clone();
    let nid = dst.add_object(Object::Null);
    map.insert(id, nid);
    let copied = copy_value(dst, src, &obj, map)?;
    dst.objects.insert(nid, copied);
    Ok(nid)
}

/// 拷贝一页（不裁剪/缩放），仅新建页面字典。用于分隔页复制。
fn copy_page_shallow(
    dst: &mut Document,
    src: &Document,
    page_id: ObjectId,
    map: &mut HashMap<ObjectId, ObjectId>,
) -> ImposeResult<ObjectId> {
    let dict_obj = src.get_object(page_id)?.clone();
    let mut pd = match dict_obj {
        Object::Dictionary(d) => d,
        _ => return Err(ImposeError::pdf("分隔页不是字典对象")),
    };
    pd.remove(b"Parent");
    let obj = copy_value(dst, src, &Object::Dictionary(pd), map)?;
    Ok(dst.add_object(obj))
}

/// 拷贝一页内容页：应用旋转 / 裁剪 / 纸张缩放。
///
/// 缩放不改动原内容流：在 Contents 前后插 `q cm` / `Q` 两个微型流。
/// 返回 `(页对象, 最终页宽, 最终页高)`，页码定位要用。
fn add_content_page(
    dst: &mut Document,
    src: &Document,
    page_id: ObjectId,
    map: &mut HashMap<ObjectId, ObjectId>,
    input: &FileInput,
    crop_rect_first: Option<(f64, f64, f64, f64)>,
) -> ImposeResult<(ObjectId, f64, f64)> {
    let mut pd = resolved_page(src, page_id)?;
    let (original, source_matrix) = source_geometry(&pd);
    pd.remove(b"Rotate");
    // 本页裁剪矩形：以第一页检测/设定的矩形为准（同一文件各页尺寸一致）
    let crop = crop_rect_first;
    // 内容空间尺寸（裁剪后）
    let (cw, ch) = match crop {
        Some((x0, y0, x1, y1)) => ((x1 - x0).abs(), (y1 - y0).abs()),
        None => original,
    };
    let rot = input.rotate.rem_euclid(360);
    // 旋转后的显示尺寸（旋转烘焙进内容变换，不再用 /Rotate）
    let (dw, dh) = if rot == 90 || rot == 270 { (ch, cw) } else { (cw, ch) };
    // 变换矩阵 cm = [a b c d e f]：内容点 → 页面点
    // 组成：裁剪平移(-x0,-y0) → 旋转（烘焙进内容）
    let (x0, y0) = crop.map(|(a, b, _, _)| (a, b)).unwrap_or((0.0, 0.0));
    let (a, b, c, d, e, f) = match rot {
        90 => (0.0, -1.0, 1.0, 0.0, -y0, cw + x0),
        180 => (-1.0, 0.0, 0.0, -1.0, cw + x0, ch + y0),
        270 => (0.0, 1.0, -1.0, 0.0, ch + y0, -x0),
        _ => (1.0, 0.0, 0.0, 1.0, -x0, -y0),
    };
    // Compose source rotation/origin with the crop and user rotation transform.
    let [sa, sb, sc, sd, se, sf] = source_matrix;
    let (a,b,c,d,e,f) = (
        a*sa+c*sb, b*sa+d*sb, a*sc+c*sd, b*sc+d*sd,
        a*se+c*sf+e, b*se+d*sf+f,
    );
    let identity = a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 && e == 0.0 && f == 0.0;

    // 深拷贝页面字典及可达对象
    let new_page_id = dst.add_object(Object::Null);
    // Back-references (e.g. annotation /P) must not recursively import the page tree.
    let previous = map.insert(page_id, new_page_id);
    let obj = copy_value(dst, src, &Object::Dictionary(pd), map)?;
    dst.objects.insert(new_page_id, obj);
    if let Some(previous) = previous { map.insert(page_id, previous); }


    // 变换微型流先加入 dst（避免与 get_object_mut 的可变借用冲突）
    let scale_streams = if !identity {
        Some((
            dst.add_object(Stream::new(
                Dictionary::new(),
                format!("q\n{a} {b} {c} {d} {e} {f} cm\n").into_bytes(),
            )),
            dst.add_object(Stream::new(Dictionary::new(), b"Q".to_vec())),
        ))
    } else {
        None
    };

    // 在 dst 的页面上做后处理
    if let Object::Dictionary(pd2) = dst.get_object_mut(new_page_id)? {
        // Source origins and rotations are baked into content; normalize page boxes.
        let set_box = |pd2: &mut lopdf::Dictionary, w: f64, h: f64| {
            let arr = || {
                Object::Array(vec![
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(w as f32),
                    Object::Real(h as f32),
                ])
            };
            pd2.set("MediaBox", arr());
            pd2.set("CropBox", arr());
        };
        // Both boxes follow the final visible size.
        set_box(pd2, dw, dh);
        // 变换：前后插微型流
        if let Some((s1, s2)) = scale_streams {
            let old_contents: Vec<Object> = match pd2.get(b"Contents") {
                Ok(Object::Reference(r)) => vec![Object::Reference(*r)],
                Ok(Object::Array(a)) => a.clone(),
                _ => vec![],
            };
            let mut arr = vec![Object::Reference(s1)];
            arr.extend(old_contents);
            arr.push(Object::Reference(s2));
            pd2.set("Contents", Object::Array(arr));
        }
    }

    let final_size = if crop.is_some() || rot != 0 {
        (dw, dh)
    } else {
        original
    };
    Ok((new_page_id, final_size.0, final_size.1))
}

/// PDF 基本信息（前端文件列表展示用）。
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct PdfInfo {
    pub pages: u32,
    /// 第一页宽 / 高（mm）
    pub width_mm: f64,
    pub height_mm: f64,
}

/// 读取 PDF 的页数与第一页尺寸。
pub fn pdf_info(bytes: &[u8]) -> ImposeResult<PdfInfo> {
    let doc = pdfium().load_pdf_from_byte_slice(bytes, None)?;
    let pages = doc.pages().len() as u32;
    if pages == 0 {
        return Ok(PdfInfo { pages: 0, width_mm: 0.0, height_mm: 0.0 });
    }
    let page = doc.pages().get(0)?;
    let (w_mm, h_mm) = (
        page.width().value as f64 / MM_TO_PT,
        page.height().value as f64 / MM_TO_PT,
    );
    Ok(PdfInfo { pages, width_mm: w_mm, height_mm: h_mm })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crop::CropSpec;
    use crate::testutil::fixture;

    fn count_pages(bytes: &[u8]) -> usize {
        let doc = Document::load_mem(bytes).unwrap();
        doc.get_pages().len()
    }

    /// 每页的内容流，用来判断「哪一页是哪一页」（同源页复制出来的流字节相同）。
    fn page_streams(bytes: &[u8]) -> Vec<Vec<u8>> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.page_iter()
            .map(|id| doc.get_page_content(id))
            .collect()
    }

    #[test]
    fn impose_counts_and_separators() {
        // 2 + 4 + 6 = 12 内容页 + 2 个间隔 × 1 张分隔页 = 14 页
        let inputs = vec![
            FileInput {
                name: "A".into(),
                source: Source::Bytes(fixture("ORDER-A-2页.pdf")),
                target_pages: 2,
                rotate: 0,
                crop: None,
            },
            FileInput {
                name: "B".into(),
                source: Source::Bytes(fixture("ORDER-B-4页.pdf")),
                target_pages: 4,
                rotate: 0,
                crop: None,
            },
            FileInput {
                name: "C".into(),
                source: Source::Bytes(fixture("ORDER-C-6页.pdf")),
                target_pages: 6,
                rotate: 0,
                crop: None,
            },
        ];
        let out = impose(
            &inputs,
            &ImposeOptions {
                sep_pages: 1,
                sep_text: None,
                page_numbers: false,
                copies: 1,
            },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(count_pages(&out), 14, "12 内容页 + 2 分隔页");

        // 盖页码：页数不变、体积不膨胀（标准字体不嵌入）
        let with_nums = impose(
            &inputs,
            &ImposeOptions {
                sep_pages: 1,
                sep_text: None,
                page_numbers: true,
                copies: 1,
            },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(count_pages(&with_nums), 14, "页码不改变页数");
        assert!(
            with_nums.len() < out.len() * 3 + 100_000,
            "盖页码后体积异常膨胀：{} → {}",
            out.len(),
            with_nums.len()
        );
        // 页码文本确实写入了，且**每个文件重新从 1 计数**：
        // 排布 = A(2) → 分隔 → B(4) → 分隔 → C(6)，内容页索引 0,1 | 3..6 | 8..13
        let doc = crate::crop::pdfium().load_pdf_from_byte_slice(&with_nums, None).unwrap();
        let text_at = |i: i32| doc.pages().get(i).unwrap().text().unwrap().all();
        let (a0, b0, c0, c_last) = (text_at(0), text_at(3), text_at(8), text_at(13));
        assert!(a0.contains('1'), "A 首页应为 1，实际：{a0:?}");
        assert!(b0.contains('1'), "B 首页应重新从 1 开始，实际：{b0:?}");
        assert!(c0.contains('1'), "C 首页应重新从 1 开始，实际：{c0:?}");
        assert!(c_last.contains('6'), "C 末页应为 6，实际：{c_last:?}");
        assert!(!c_last.contains("12"), "不应再跨文件连续编号，实际：{c_last:?}");
    }

    #[test]
    fn impose_rotate_and_manual_crop() {
        let inputs = vec![FileInput {
            name: "A".into(),
            source: Source::Bytes(fixture("ORDER-A-2页.pdf")),
            target_pages: 5, // 凑页：2 页源 → 5 页输出
            rotate: 90,
            crop: Some(CropSpec::Manual {
                top_mm: 10.0,
                bottom_mm: 10.0,
                left_mm: 10.0,
                right_mm: 10.0,
            }),
        }];
        let out = impose(
            &inputs,
            &ImposeOptions {
                sep_pages: 0,
                sep_text: None,
                page_numbers: false,
                copies: 1,
            },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(count_pages(&out), 5);
        // 旋转烘焙进内容变换：无 /Rotate；裁剪 10mm 后竖页转 90° → 横向显示尺寸
        let doc = Document::load_mem(&out).unwrap();
        let first = doc.get_pages().values().next().unwrap().clone();
        let dict = doc.get_object(first).unwrap().clone();
        if let Object::Dictionary(d) = dict {
            assert!(d.get(b"Rotate").is_err(), "不应再写 /Rotate");
            let mb = d.get(b"MediaBox").cloned().unwrap();
            if let Object::Array(v) = mb {
                let nums: Vec<f64> = v
                    .iter()
                    .map(|o| match o {
                        Object::Real(x) => *x as f64,
                        Object::Integer(i) => *i as f64,
                        _ => 0.0,
                    })
                    .collect();
                // A4(595.28×841.89) 四边各裁 10mm → 538.59×785.20，转 90° 后横置
                assert!((nums[2] - 785.20).abs() < 0.5, "宽应约 785.20: {:?}", nums);
                assert!((nums[3] - 538.59).abs() < 0.5, "高应约 538.59: {:?}", nums);
            } else {
                panic!("MediaBox 不是数组");
            }
        } else {
            panic!("页面不是字典");
        }
    }

    /// 箱子标签要一箱两份：3 页 × 每页 2 份 → 1,1,2,2,3,3（不是整份轮转的 1,2,3,1,2,3）。
    #[test]
    fn copies_repeat_each_page_consecutively() {
        let inputs = vec![FileInput {
            name: "C".into(),
            source: Source::Bytes(fixture("ORDER-B-4页.pdf")),
            target_pages: 3,
            rotate: 0,
            crop: None,
        }];
        let out = impose(
            &inputs,
            &ImposeOptions { copies: 2, ..Default::default() },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(count_pages(&out), 6, "3 页 × 2 份 = 6 页");

        let s = page_streams(&out);
        assert_eq!(s[0], s[1], "第 1、2 页应是同一页");
        assert_eq!(s[2], s[3], "第 3、4 页应是同一页");
        assert_eq!(s[4], s[5], "第 5、6 页应是同一页");
        assert_ne!(s[0], s[2], "第 1 页和第 2 页必须不同");
        assert_ne!(s[2], s[4], "第 2 页和第 3 页必须不同");
    }

    /// 份数与「页数」叠加：凑页轮转时每页也要各印 copies 份。
    #[test]
    fn copies_combine_with_padding() {
        let inputs = vec![FileInput {
            name: "C".into(),
            source: Source::Bytes(fixture("ORDER-B-4页.pdf")),
            target_pages: 6,
            rotate: 0,
            crop: None,
        }];
        let out = impose(
            &inputs,
            &ImposeOptions { copies: 2, ..Default::default() },
            &|_, _| {},
        )
        .unwrap();
        assert_eq!(count_pages(&out), 12, "6 页 × 2 份 = 12 页");
        let s = page_streams(&out);
        // 4 页源轮转凑到 6 页：0,1,2,3,0,1；再每页两份
        assert_eq!(s[0], s[1]);
        assert_eq!(s[2], s[3]);
        assert_eq!(s[8], s[0], "第 6 页（索引 5）又轮到源第 1 页");
    }

    #[test]
    fn impose_130k_pages_under_3s() {
        let inputs = vec![FileInput {
            name: "P".into(),
            source: Source::Bytes(fixture("PROBE-A4.pdf")),
            target_pages: 130_002,
            rotate: 0,
            crop: None,
        }];
        let t = Instant::now();
        let out = impose(&inputs, &ImposeOptions::default(), &|_, _| {}).unwrap();
        let elapsed = t.elapsed();
        assert_eq!(count_pages(&out), 130_002);
        eprintln!("13 万页总耗时（含写出）: {:?}", elapsed);
        assert!(elapsed.as_secs() < 3, "13 万页应在 3 秒内完成，实际 {:?}", elapsed);
    }

    /// 分隔页按 (尺寸, 文字) 缓存：60 个文件只付一次字体嵌入代价。
    /// 改前每张都重读 19.7MB 字体 + 重存 20MB PDF，60 文件要 78 秒。
    #[test]
    fn separators_are_cached_across_files() {
        let one = fixture("ORDER-B-4页.pdf");
        let inputs: Vec<FileInput> = (0..60)
            .map(|i| FileInput {
                name: format!("f{i}"),
                source: Source::Bytes(one.clone()),
                target_pages: 4,
                rotate: 0,
                crop: None,
            })
            .collect();
        let t = Instant::now();
        let out = impose(
            &inputs,
            &ImposeOptions { sep_pages: 1, ..Default::default() },
            &|_, _| {},
        )
        .unwrap();
        let elapsed = t.elapsed();
        assert_eq!(count_pages(&out), 60 * 4 + 59, "每个文件后应插 1 张分隔页");
        assert!(elapsed.as_secs() < 5, "分隔页应命中缓存，实际 {:?}", elapsed);
    }

    /// 页码走 lopdf 追加内容流：5000 页应是 O(n) 的毫秒级。
    /// 改前是 pdfium 全文档往返，5000 页要 505 秒且近似平方增长。
    #[test]
    fn page_numbers_stay_linear() {
        let inputs = vec![FileInput {
            name: "P".into(),
            source: Source::Bytes(fixture("PROBE-A4.pdf")),
            target_pages: 5_000,
            rotate: 0,
            crop: None,
        }];
        let t = Instant::now();
        let out = impose(
            &inputs,
            &ImposeOptions { page_numbers: true, ..Default::default() },
            &|_, _| {},
        )
        .unwrap();
        let elapsed = t.elapsed();
        assert_eq!(count_pages(&out), 5_000);
        assert!(elapsed.as_secs() < 3, "5000 页盖页码应远低于 3 秒，实际 {:?}", elapsed);
    }

    /// 流式写出与内存输出等价（页数一致、字节一致）。
    #[test]
    fn impose_to_writer_matches_vec() {
        let inputs = vec![FileInput {
            name: "A".into(),
            source: Source::Bytes(fixture("ORDER-A-2页.pdf")),
            target_pages: 2,
            rotate: 0,
            crop: None,
        }];
        let opts = ImposeOptions { sep_pages: 1, page_numbers: true, ..Default::default() };
        let mem = impose(&inputs, &opts, &|_, _| {}).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let stats = impose_to(&inputs, &opts, &|_, _| {}, &mut buf).unwrap();
        assert_eq!(stats.pages, count_pages(&mem));
        assert_eq!(buf, mem, "两个写出路径应产生同样字节");
    }

    /// 损坏的 PDF（页面引用了不存在的对象）应返回可读错误，而不是 panic。
    ///
    /// release 是 `panic = "abort"`，一次 panic 就等于进程消失、用户没有任何提示。
    #[test]
    fn broken_reference_returns_error() {
        use lopdf::dictionary;

        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0), Object::Integer(0),
                Object::Integer(595), Object::Integer(842),
            ],
            // 故意指向一个不存在的对象
            "Contents" => Object::Reference((999_u32, 0_u16)),
        };
        let page_id = doc.add_object(page);
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => Object::Integer(1),
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let inputs = vec![FileInput {
            name: "broken".into(),
            source: Source::Bytes(bytes),
            target_pages: 1,
            rotate: 0,
            crop: None,
        }];
        let err = impose(&inputs, &ImposeOptions::default(), &|_, _| {}).unwrap_err();
        assert!(
            err.to_string().contains("结构损坏"),
            "应返回可读错误而不是 panic，实际: {err}"
        );
    }


    fn inherited_fixture(rotation: i64) -> Vec<u8> {
        let mut doc = Document::load_mem(&fixture("PROBE-A4.pdf")).unwrap();
        let pid = doc.page_iter().next().unwrap();
        let parent = doc.get_object(pid).unwrap().as_dict().unwrap()
            .get(b"Parent").unwrap().as_reference().unwrap();
        for key in [b"MediaBox".as_slice(), b"Resources", b"CropBox"] {
            let value = doc.get_object_mut(pid).unwrap().as_dict_mut().unwrap().remove(key);
            if let Some(value) = value {
                doc.get_object_mut(parent).unwrap().as_dict_mut().unwrap().set(key.to_vec(), value);
            }
        }
        doc.get_object_mut(parent).unwrap().as_dict_mut().unwrap().set("Rotate", rotation);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn inherited_resources_and_rotations_preserve_rendering() {
        use pdfium_render::prelude::*;
        for rotation in [0, 90, 180, 270] {
            let bytes = inherited_fixture(rotation);
            let input = FileInput::from_bytes("inherited", bytes.clone());
            let out = impose(&[input], &ImposeOptions::default(), &|_,_| {}).unwrap();
            let render = |data: &[u8]| {
                let doc = pdfium().load_pdf_from_byte_slice(data, None).unwrap();
                let image = doc.pages().get(0).unwrap()
                    .render_with_config(&PdfRenderConfig::new().set_target_width(400)).unwrap()
                    .as_image().unwrap().to_rgb8();
                image
            };
            let before = render(&bytes);
            let after = render(&out);
            assert_eq!(before.dimensions(), after.dimensions(), "rotation {rotation}");
            let mean_error = before.as_raw().iter().zip(after.as_raw())
                .map(|(a,b)| (*a as f64-*b as f64).abs()).sum::<f64>() / before.as_raw().len() as f64;
            assert!(mean_error < 1.0, "rotation {rotation}: pixel error {mean_error}");
        }
    }

    #[test]
    fn inherited_rotation_combines_with_user_rotation_and_crop() {
        for original in [0, 90, 180, 270] {
            for extra in [0, 90, 180, 270] {
                let bytes = inherited_fixture(original);
                let src = Document::load_mem(&bytes).unwrap();
                let pid = src.page_iter().next().unwrap();
                let visible = source_geometry(&resolved_page(&src, pid).unwrap()).0;
                let mut input = FileInput::from_bytes("rotated", bytes);
                input.rotate = extra;
                input.crop = Some(CropSpec::Rect { x0: 10., y0: 20., x1: visible.0-30., y1: visible.1-40. });
                let out = impose(&[input], &ImposeOptions::default(), &|_,_| {}).unwrap();
                let info = pdf_info(&out).unwrap();
                let expected = display_size((visible.0-40., visible.1-60.), None, extra);
                assert!((info.width_mm*MM_TO_PT-expected.0).abs()<0.1);
                assert!((info.height_mm*MM_TO_PT-expected.1).abs()<0.1);
            }
        }
    }

    #[test]
    fn copying_cyclic_objects_preserves_back_references() {
        let mut src = Document::with_version("1.4");
        let a = src.add_object(Object::Null);
        let b = src.add_object(Object::Array(vec![Object::Reference(a)]));
        src.objects.insert(a, Object::Array(vec![Object::Reference(b)]));
        let mut dst = new_document();
        let mut map = HashMap::new();
        let copied = copy_object(&mut dst, &src, a, &mut map).unwrap();
        assert_eq!(map.len(), 2);
        let next = dst.get_object(copied).unwrap().as_array().unwrap()[0].as_reference().unwrap();
        let back = dst.get_object(next).unwrap().as_array().unwrap()[0].as_reference().unwrap();
        assert_eq!(back, copied);
    }

    #[test]
    fn cyclic_page_parent_returns_error() {
        let mut doc = Document::load_mem(&fixture("PROBE-A4.pdf")).unwrap();
        let pid = doc.page_iter().next().unwrap();
        doc.get_object_mut(pid).unwrap().as_dict_mut().unwrap().set("Parent", Object::Reference(pid));
        assert!(resolved_page(&doc, pid).unwrap_err().to_string().contains("循环"));
    }

    #[test]
    fn annotation_page_back_reference_does_not_import_source_tree() {
        let mut doc = Document::load_mem(&fixture("PROBE-A4.pdf")).unwrap();
        let pid = doc.page_iter().next().unwrap();
        let mut annot = Dictionary::new();
        annot.set("Type", Object::Name(b"Annot".to_vec()));
        annot.set("Subtype", Object::Name(b"Text".to_vec()));
        annot.set("P", Object::Reference(pid));
        let aid = doc.add_object(annot);
        doc.get_object_mut(pid).unwrap().as_dict_mut().unwrap()
            .set("Annots", vec![Object::Reference(aid)]);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let out = impose(&[FileInput::from_bytes("annotation", bytes)], &ImposeOptions::default(), &|_,_| {}).unwrap();
        let result = Document::load_mem(&out).unwrap();
        let page = result.page_iter().next().unwrap();
        let annotation = result.get_object(page).unwrap().as_dict().unwrap()
            .get(b"Annots").unwrap().as_array().unwrap()[0].as_reference().unwrap();
        assert_eq!(result.get_object(annotation).unwrap().as_dict().unwrap()
            .get(b"P").unwrap().as_reference().unwrap(), page);
        assert_eq!(result.page_iter().count(), 1);
    }

}
