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
use std::collections::HashMap;
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

#[derive(Debug, Clone, Default)]
pub struct ImposeOptions {

    /// 每个文件（第一个除外）前插入的分隔页数量
    pub sep_pages: u32,
    /// 分隔页文字，默认「这是分隔线」
    pub sep_text: Option<String>,
    /// 在每个文件的内容页右下角盖页码 1,2,3…（每个文件重新从 1 开始，分隔页不编号）
    pub page_numbers: bool,
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
    let mut total: usize = inputs.iter().map(|i| (i.target_pages as usize).max(1)).sum();
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
        let first_dict = page_dictionary(&src, page_ids[0])?;
        let first_size = page_display_size(first_dict);
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
        for k in 0..need {
            // 循环复制：凑页时按整个文档轮转（与 Streamlit 版一致），截取时天然只取前 N 页
            let pid = page_ids[k % page_ids.len()];
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

/// 页面原始尺寸（未裁剪、未旋转）。
fn page_display_size(dict: &Dictionary) -> (f64, f64) {
    let (x0, y0, x1, y1) = mediabox_of(dict);
    ((x1 - x0).abs(), (y1 - y0).abs())
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
    let copied = copy_value(dst, src, &obj, map)?;
    let nid = dst.add_object(copied);
    map.insert(id, nid);
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
    let dict_obj = src.get_object(page_id)?.clone();
    let mut pd = match dict_obj {
        Object::Dictionary(d) => d,
        _ => return Err(ImposeError::pdf("页面不是字典对象")),
    };
    pd.remove(b"Parent");
    pd.remove(b"Rotate");

    let original = page_display_size(&pd);
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
    let identity = a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 && e == 0.0 && f == 0.0;

    // 深拷贝页面字典及可达对象
    let obj = copy_value(dst, src, &Object::Dictionary(pd), map)?;
    let new_page_id = dst.add_object(obj);

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
        // 页面框：有裁剪设为裁剪后尺寸（原点归零）；纯旋转设为显示尺寸；否则沿用原页
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
        // 页面框：有裁剪或旋转时都设为显示尺寸（dw/dh 已含裁剪与 90/270 的宽高互换）；
        // 否则沿用原页框。之前 crop+rotate 会漏掉宽高互换，导致旋转后被裁切。
        if crop.is_some() || rot != 0 {
            set_box(pd2, dw, dh);
        }
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


}
