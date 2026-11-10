//! 只读云表 + 手机/电脑本地出货台账。空单号不是删除票据的指令。
use super::*;

fn same_batch(a: &MobileRow, b: &MobileRow) -> bool {
    equivalent(a, b)
        && a.qty == b.qty
        && match (a.source_row, b.source_row) {
            (Some(x), Some(y)) => x == y,
            _ => a.seq == 0 || b.seq == 0 || a.seq == b.seq,
        }
}
fn unique_match(rows: &[MobileRow], row: &MobileRow) -> Option<usize> {
    let exact: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| same_batch(r, row))
        .map(|(i, _)| i)
        .collect();
    if let [index] = exact.as_slice() {
        return Some(*index);
    }
    if !exact.is_empty() {
        return None;
    }
    let by_cargo: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| equivalent(r, row) && r.qty == row.qty)
        .map(|(i, _)| i)
        .collect();
    if let [index] = by_cargo.as_slice() {
        Some(*index)
    } else {
        None
    }
}
fn is_history(row: &MobileRow) -> bool {
    row.issues.iter().any(|i| i == "history_archive")
}
/// 写前自愈：旧版同步遗留的同单号+同货+同量历史副本，有现行行时先把副本的
/// 单号/勾选并回现行行再丢弃；没有现行行时保留一条（已出票据不能丢），多余副本只留最早一条。
/// 只对比 history_archive 副本，原表本来就有的两行同货同量不会被合并。
fn dedupe_history(rows: &mut Vec<MobileRow>) {
    let mut drop = vec![false; rows.len()];
    for i in 0..rows.len() {
        if !is_history(&rows[i]) {
            continue;
        }
        let keeper = (0..rows.len()).find(|&j| {
            j != i
                && !drop[j]
                && rows[j].waybill.trim() == rows[i].waybill.trim()
                && rows[j].qty == rows[i].qty
                && equivalent(&rows[j], &rows[i])
                && (!is_history(&rows[j]) || j < i)
        });
        let Some(j) = keeper else { continue };
        let (waybill, checked, sync_id) = (
            rows[i].waybill.clone(),
            rows[i].checked,
            rows[i].sync_id.clone(),
        );
        if rows[j].waybill.trim().is_empty() {
            rows[j].waybill = waybill;
        }
        rows[j].checked |= checked;
        if rows[j].sync_id.is_empty() {
            rows[j].sync_id = sync_id;
        }
        drop[i] = true;
    }
    let mut index = 0;
    rows.retain(|_| {
        let dropped = drop[index];
        index += 1;
        !dropped
    });
}
fn restore(rows: &mut Vec<MobileRow>, history: &[MobileRow]) {
    for old in history {
        if old.waybill.trim().is_empty() {
            continue;
        }
        if let Some(index) = unique_match(rows, old) {
            let row = &mut rows[index];
            if row.waybill.trim().is_empty() {
                row.waybill = old.waybill.clone();
                add_manual(row, "waybill");
            }
            if row.waybill == old.waybill && !row.manual_fields.iter().any(|f| f == "checked") {
                row.checked |= old.checked;
            }
            if row.sync_id.is_empty() {
                row.sync_id = old.sync_id.clone();
            }
        } else if old.checked
            && !rows.iter().any(|r| {
                equivalent(r, old)
                    && (!r.issues.iter().any(|i| i == "history_archive")
                        || notebook_identity(r) == notebook_identity(old))
            })
        {
            let mut row = old.clone();
            if !row.issues.iter().any(|i| i == "history_archive") {
                row.issues.push("history_archive".into());
            }
            rows.push(row);
        }
    }
}

pub(super) fn exchange_ledger(
    shared: &MobileShared,
    request: Request,
) -> Result<Response, (u16, String)> {
    if request.device_id.is_empty()
        || request.device_id.len() > 100
        || request.request_id.is_empty()
        || request.request_id.len() > 150
        || request.rows.len() > 20000
        || request.operations.len() > 50000
        || request.waybill_notes.len() > 20000
    {
        return Err((400, "出货台账同步数据不合法或超出范围".into()));
    }
    let mut ids = BTreeSet::new();
    for row in &request.rows {
        if row.qty < 0
            || row.qty > 1_000_000_000
            || row.sync_id.len() > 100
            || (!row.sync_id.is_empty() && !ids.insert(row.sync_id.clone()))
        {
            return Err((400, "台账记录ID或数量无效".into()));
        }
    }
    let mut inner = shared
        .inner
        .lock()
        .map_err(|_| (500, "手机状态锁异常".into()))?;
    let db = shared.db.lock().map_err(|_| (500, "数据库锁异常".into()))?;
    let mut state = load_state(&db).map_err(|e| (500, e))?;
    if let Some(peer) = state.peers.get(&request.device_id) {
        if peer.request_id == request.request_id && peer.response.mode == "shipment-ledger" {
            return Ok(peer.response.clone());
        }
    }
    if state.computer_id.is_empty() {
        state.computer_id = uid();
    }
    let peer = state
        .peers
        .get(&request.device_id)
        .cloned()
        .unwrap_or_default();
    let mut rows = inner.rows.clone();
    // 先取完整电脑存档和上次台账，不能拿手机半张票覆盖完整票据。
    let history = state.records.clone();
    restore(&mut rows, &history);
    restore(&mut rows, &request.rows);
    dedupe_history(&mut rows);
    if rows.is_empty() {
        return Err((409, "电脑尚未载入云表或出货台账，手机原数据已保留".into()));
    }
    let mut used = BTreeSet::new();
    for row in &mut rows {
        if row.sync_id.is_empty() || !used.insert(row.sync_id.clone()) {
            row.sync_id = uid();
            used.insert(row.sync_id.clone());
        }
        row.source_identity = identity(row);
    }
    let mut conflicts = Vec::new();
    let mut phone_view = rows.clone();
    for phone in &request.rows {
        let found = rows
            .iter()
            .position(|r| {
                !phone.sync_id.is_empty() && r.sync_id == phone.sync_id && same_batch(r, phone)
            })
            .or_else(|| unique_match(&rows, phone));
        let Some(index) = found else {
            continue;
        };
        let computer = rows[index].clone();
        let base = peer
            .base
            .iter()
            .find(|r| r.sync_id == computer.sync_id)
            .or_else(|| peer.base.iter().find(|r| same_batch(r, &computer)));
        for field in ["waybill", "checked"] {
            let pv = business(phone);
            let cv = business(&computer);
            let a = &pv[field];
            let b = &cv[field];
            let key = format!("{}:{field}", computer.sync_id);
            let choice = request
                .resolutions
                .get(&key)
                .filter(|r| r.phone == *a && r.computer == *b);
            let manual = phone.manual_fields.iter().any(|f| f == field);
            let pending = peer
                .conflicts
                .iter()
                .any(|c| c.record_id == computer.sync_id && c.field == field);
            let bv = base.map(business);
            let selected = if a == b {
                Some(b)
            } else if let Some(resolution) = choice {
                match resolution.choice.as_str() {
                    "phone" => Some(a),
                    "computer" => Some(b),
                    _ => None,
                }
            } else if pending {
                None
            } else if let Some(base) = &bv {
                if a == &base[field] {
                    Some(b)
                } else if b == &base[field] {
                    Some(a)
                } else {
                    None
                }
            } else if field == "waybill" && b.as_str() == Some("") && a.as_str() != Some("") {
                Some(a)
            } else if !manual {
                Some(b)
            } else if !computer.manual_fields.iter().any(|f| f == field) {
                Some(a)
            } else {
                None
            };
            if let Some(value) = selected {
                // 云表删号或旧手机空号均不能自动清除已有台账单号。
                if field == "waybill"
                    && value.as_str() == Some("")
                    && !computer.waybill.is_empty()
                    && choice.is_none()
                {
                    continue;
                }
                if field == "waybill" {
                    rows[index].waybill = value.as_str().unwrap_or("").into();
                } else {
                    rows[index].checked = value.as_bool().unwrap_or(false);
                }
                if a != b || manual {
                    add_manual(&mut rows[index], field);
                }
                if field == "waybill" {
                    phone_view[index].waybill = rows[index].waybill.clone();
                } else {
                    phone_view[index].checked = rows[index].checked;
                }
                phone_view[index].manual_fields = rows[index].manual_fields.clone();
            } else {
                conflicts.push(Conflict {
                    record_id: computer.sync_id.clone(),
                    field: field.into(),
                    phone: a.clone(),
                    computer: b.clone(),
                    name: computer.name.clone(),
                    inbound: computer.inbound.clone(),
                    source_row: computer.source_row,
                });
                if field == "waybill" {
                    phone_view[index].waybill = phone.waybill.clone();
                } else {
                    phone_view[index].checked = phone.checked;
                }
            }
        }
    }
    // 多个不同记忆号码不自动选第一个；只采用唯一对应、唯一号码。
    let mut note_numbers: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for note in &request.waybill_notes {
        if note.waybill.len() > 100 || note.target_identity.len() > 2000 {
            return Err((400, "单号记忆内容超出范围".into()));
        }
        if note.waybill.trim().is_empty() {
            continue;
        }
        if let [index] = note_candidates(&rows, note).as_slice() {
            note_numbers
                .entry(*index)
                .or_default()
                .insert(note.waybill.trim().into());
        }
    }
    for (index, numbers) in note_numbers {
        if rows[index].waybill.is_empty() && numbers.len() == 1 {
            rows[index].waybill = numbers.first().unwrap().clone();
            add_manual(&mut rows[index], "waybill");
            phone_view[index].waybill = rows[index].waybill.clone();
        }
    }
    let changed = rows.len() != inner.rows.len()
        || rows.iter().zip(&inner.rows).any(|(a, b)| {
            a.sync_id != b.sync_id || FIELDS.iter().any(|f| business(a)[*f] != business(b)[*f])
        });
    if changed {
        state.revision += 1;
    }
    let source = inner
        .rows
        .first()
        .map(|r| r.source_id.clone())
        .unwrap_or_default();
    for list in [&mut rows, &mut phone_view] {
        for row in list {
            row.master_revision = state.revision;
            row.source_id = source.clone();
            row.known_fields = FIELDS.iter().map(|f| (*f).into()).collect();
            row.record_id =
                json!([row.waybill.trim(), row.inbound.trim(), row.name.trim()]).to_string();
            row.blocked = !row
                .issues
                .iter()
                .any(|i| i == "special_amazon" || i == "special_shenzhen")
                && (row.inbound.trim().is_empty() || row.waybill.trim().is_empty());
        }
    }
    let response = Response {
        mode: "shipment-ledger".into(),
        protocol: 1,
        computer_id: state.computer_id.clone(),
        revision: state.revision,
        rows: phone_view,
        conflicts: conflicts.clone(),
        operations: vec![],
    };
    state.mode = "shipment-ledger".into();
    state.records = rows.clone();
    state.updated_at = now_ms().to_string();
    state.peers.insert(
        request.device_id.clone(),
        Peer {
            base: rows.clone(),
            conflicts,
            request_id: request.request_id.clone(),
            response: response.clone(),
        },
    );
    persist(
        &db,
        &state,
        &request.device_id,
        &json!({"mode":"shipment-ledger","request":request,"at":now_ms()}),
    )
    .map_err(|e| (500, format!("台账保存失败，原数据未覆盖：{e}")))?;
    inner.rows = rows;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shared(rows: Vec<MobileRow>) -> MobileShared {
        let s = crate::mobile::new_shared();
        s.inner.lock().unwrap().rows = rows;
        s
    }
    fn row(index: u32, qty: i64) -> MobileRow {
        MobileRow {
            seq: index + 1,
            source_row: Some(382 + index),
            inbound: format!("I{index}"),
            name: format!("商品{index}"),
            waybill: "6812706003".into(),
            destination: "休斯顿".into(),
            customer: "杨总".into(),
            qty,
            ..Default::default()
        }
    }
    fn request(id: &str, rows: Vec<MobileRow>) -> Request {
        Request {
            mode: "shipment-ledger".into(),
            device_id: "phone".into(),
            request_id: id.into(),
            rows,
            ..Default::default()
        }
    }
    #[test]
    fn legacy_duplicate_history_copies_collapse_and_do_not_regrow() {
        let live = row(0, 4);
        let mut all = vec![live.clone()];
        for i in 0..4 {
            let mut copy = live.clone();
            copy.seq = 60 + i;
            copy.source_row = None;
            copy.checked = true;
            copy.issues.push("history_archive".into());
            all.push(copy);
        }
        let s = shared(all);
        let first = exchange_ledger(&s, request("heal", vec![])).unwrap();
        assert_eq!(first.rows.len(), 1);
        assert!(!first.rows[0].issues.iter().any(|i| i == "history_archive"));
        assert!(first.rows[0].checked, "历史副本的已出状态要补到现行行");
        let second = exchange_ledger(&s, request("heal2", first.rows)).unwrap();
        assert_eq!(second.rows.len(), 1);
        assert_eq!(
            s.db.lock().unwrap().ledger_row_count().unwrap(),
            1,
            "台账不能累积同货同量的历史副本"
        );
    }
    #[test]
    fn dedupe_keeps_completed_ticket_without_live_row_and_split_batches() {
        let mut gone = row(0, 6);
        gone.checked = true;
        gone.issues.push("history_archive".into());
        let mut gone_copy = gone.clone();
        gone_copy.seq = 70;
        gone_copy.sync_id.clear();
        let mut a = row(1, 20);
        a.sync_id.clear();
        let mut b = row(2, 15);
        b.sync_id.clear();
        let s = shared(vec![gone, gone_copy, a, b]);
        let result = exchange_ledger(&s, request("keep", vec![])).unwrap();
        assert_eq!(result.rows.len(), 3, "已出票据留一条，20/15 分批都保留");
        assert_eq!(result.rows.iter().map(|r| r.qty).sum::<i64>(), 41);
    }
    #[test]
    fn cloud_cleared_number_and_phone_half_ticket_restore_ten_rows_twenty_one_pieces() {
        let initial: Vec<_> = [4, 1, 6, 1, 2, 1, 2, 2, 1, 1]
            .into_iter()
            .enumerate()
            .map(|(i, q)| {
                let mut r = row(i as u32, q);
                r.checked = true;
                r
            })
            .collect();
        let s = shared(initial);
        let first = exchange_ledger(&s, request("first", vec![])).unwrap();
        let phone_half = first.rows[5..].to_vec();
        assert_eq!(phone_half.iter().map(|r| r.qty).sum::<i64>(), 7);
        for r in &mut s.inner.lock().unwrap().rows {
            r.waybill.clear();
            r.checked = false;
            r.sync_id.clear();
            r.manual_fields.clear();
        }
        let restored = exchange_ledger(&s, request("cleared", phone_half)).unwrap();
        assert_eq!(restored.rows.len(), 10);
        assert_eq!(restored.rows.iter().map(|r| r.qty).sum::<i64>(), 21);
        assert!(
            restored
                .rows
                .iter()
                .all(|r| r.waybill == "6812706003" && r.checked)
        );
        assert_eq!(s.db.lock().unwrap().ledger_row_count().unwrap(), 10);
        let again = exchange_ledger(&s, request("repeat", restored.rows)).unwrap();
        assert_eq!(again.rows.len(), 10);
    }
    #[test]
    fn phone_ships_and_computer_undo_both_sync_without_changing_cloud_cargo() {
        let s = shared(vec![row(0, 4)]);
        let first = exchange_ledger(&s, request("first", vec![])).unwrap();
        let mut phone = first.rows;
        phone[0].checked = true;
        phone[0].manual_fields.push("checked".into());
        phone[0].customer = "手机旧资料".into();
        let shipped = exchange_ledger(&s, request("ship", phone)).unwrap();
        assert!(s.inner.lock().unwrap().rows[0].checked);
        assert_eq!(shipped.rows[0].customer, "杨总");
        s.inner.lock().unwrap().rows[0].checked = false;
        let undone = exchange_ledger(&s, request("undo", shipped.rows)).unwrap();
        assert!(!undone.rows[0].checked);
    }
    #[test]
    fn two_computers_keep_completed_ticket_even_when_cloud_rows_removed() {
        let company = shared(vec![{
            let mut r = row(0, 4);
            r.checked = true;
            r
        }]);
        let carried = exchange_ledger(&company, request("company", vec![])).unwrap();
        let home = shared(vec![row(1, 1)]);
        let synced = exchange_ledger(&home, request("home", carried.rows)).unwrap();
        assert!(
            synced
                .rows
                .iter()
                .any(|r| r.name == "商品0" && r.qty == 4 && r.checked)
        );
        home.inner.lock().unwrap().rows.clear();
        let restored = exchange_ledger(&home, request("offline", synced.rows)).unwrap();
        assert!(restored.rows.iter().any(|r| r.name == "商品0" && r.checked));
    }
    #[test]
    fn old_35_box_parent_never_duplicates_20_and_15_split_batches() {
        let mut a = row(0, 20);
        a.waybill.clear();
        let mut b = a.clone();
        b.seq = 2;
        b.source_row = Some(400);
        b.qty = 15;
        let mut old = a.clone();
        old.seq = 0;
        old.source_row = None;
        old.qty = 35;
        old.checked = true;
        old.waybill = "OLD".into();
        let s = shared(vec![a, b]);
        let result = exchange_ledger(&s, request("split", vec![old])).unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows.iter().map(|r| r.qty).sum::<i64>(), 35);
    }
    #[test]
    fn simultaneous_waybill_edits_stay_conflicted_until_explicit_resolution() {
        let s = shared(vec![row(0, 4)]);
        let first = exchange_ledger(&s, request("first", vec![])).unwrap();
        let mut phone = first.rows;
        phone[0].waybill = "PHONE".into();
        phone[0].manual_fields.push("waybill".into());
        s.inner.lock().unwrap().rows[0].waybill = "COMPUTER".into();
        let conflict = exchange_ledger(&s, request("conflict", phone)).unwrap();
        assert_eq!(conflict.conflicts.len(), 1);
        assert_eq!(conflict.rows[0].waybill, "PHONE");
        assert_eq!(s.inner.lock().unwrap().rows[0].waybill, "COMPUTER");
        let c = &conflict.conflicts[0];
        let mut resolve = request("resolve", conflict.rows.clone());
        resolve.resolutions.insert(
            format!("{}:{}", c.record_id, c.field),
            Resolution {
                choice: "phone".into(),
                phone: c.phone.clone(),
                computer: c.computer.clone(),
            },
        );
        let resolved = exchange_ledger(&s, resolve).unwrap();
        assert!(resolved.conflicts.is_empty());
        assert_eq!(resolved.rows[0].waybill, "PHONE");
    }
}
