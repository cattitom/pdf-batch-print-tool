if (window.parent && window.parent !== window) document.documentElement.classList.add("in-shell");
/* 手机出货（A 方案）：票列表 → 单据页（单据头 + 行项目 + 确认出货）。无框架。 */
(function () {
  "use strict";

  var transport = window.shippingTransport;
  var apiFetch = function (path, options) { return transport ? transport.request(path, options) : fetch(url(path), options); };
  function syncText() { return transport && transport.status() || nowText(); }
  var TOKEN = new URLSearchParams(location.search).get("t") || "";
  var CHECK =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 13l4 4L19 7"/></svg>';

  var rows = [];
  var view = "list";
  var cur = "";
  var filter = "all";
  var layout = "board";
  var q = "";
  var docQ = "";
  var saving = {};
  var pollTimer = null;
  var cloudRefreshing = false;
  var receiving = false;

  var $ = function (id) {
    return document.getElementById(id);
  };
  var $viewList = $("view-list");
  var $viewDoc = $("view-doc");
  var $tickets = $("tickets");
  var $lines = $("lines");
  var $sub = $("sub");
  var $done = $("doneQty");
  var $left = $("leftQty");
  var $sync = $("sync");
  var $q = $("q");
  var $dq = $("dq");
  var $seg = $("seg");
  var $docNo = $("docNo");
  var $docSub = $("docSub");
  var $docTag = $("docTag");
  var $docBar = $("docBar");
  var $docPct = $("docPct");
  var $docDone = $("docDone");
  var $docLeft = $("docLeft");
  var $docDiff = $("docDiff");
  var $footDone = $("footDone");
  var $footTotal = $("footTotal");
  var $receive = $("receive");
  var $toast = $("toast");

  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  function keyOf(r) {
    return r.syncId || r.waybill + "|" + r.inbound + "|" + r.name + (!r.inbound ? '|'+(r.seq||0)+'|'+r.qty+'|'+r.customer : '');
  }

  function url(path) {
    var sep = path.indexOf("?") >= 0 ? "&" : "?";
    return path + sep + "t=" + encodeURIComponent(TOKEN);
  }

  function toast(msg) {
    $toast.textContent = msg;
    $toast.classList.add("show");
    clearTimeout(toast._t);
    toast._t = setTimeout(function () {
      $toast.classList.remove("show");
    }, 2200);
  }

  function setSync(text, isErr) {
    $sync.textContent = text;
    $sync.classList.toggle("err", !!isErr);
  }

  // 导出已显示的本地数据，不发请求、不修改勾选；保留重复行。
  function exportText(list, scope, info, at) {
    function cell(value) { return String(value == null ? "" : value).replace(/[\t\r\n]+/g, " "); }
    var total = sumQty(list), done = sumQty(list.filter(function (r) { return r.checked; }));
    var lines = ["出货清单导出 · " + scope,
      "来源电脑：" + cell(info.source), "导出时间：" + new Date(at).toLocaleString(),
      "数据保存时间：" + (info.cachedAt ? new Date(info.cachedAt).toLocaleString() : "未知"),
      "连接状态：" + (info.offline ? "离线缓存" : "以手机当前显示数据为准") + " · 待同步操作 " + (info.pending || 0) + " 条",
      list.length + " 行 · " + total + " 件 · 已出 " + done + " 件 · 未出 " + (total - done) + " 件",
      "单号\t入库号\t商品\t客户\t数量\t出货状态\t存档\t标记"];
    list.forEach(function (r) {
      var issues = r.issues || [];
      lines.push([r.waybill, r.inbound, r.name, r.customer, r.qty, r.checked ? "已出货" : "未出货",
        issues.indexOf("history_archive") >= 0 || issues.indexOf("waybill_archive") >= 0 ? "含存档" : "", issues.join("、")].map(cell).join("\t"));
    });
    return lines.join("\n");
  }

  var exportSnapshot = [], exportMeta = {}, exportTicket = "";
  function updateExport() {
    var scope = $("export-scope").value;
    var selected = exportSnapshot.filter(function (r) {
      return scope === "done" ? r.checked : scope === "ticket" ? (r.waybill || "未识别单号") === exportTicket : true;
    });
    $("export-text").value = selected.length ? exportText(selected, scope === "done" ? "已出货明细" : scope === "ticket" ? "当前票 " + exportTicket : "全部清单", exportMeta, Date.now()) : "";
    $("export-summary").textContent = selected.length + " 行 · " + sumQty(selected) + " 件（不受列表搜索和筛选影响）";
    $("export-copy").disabled = !selected.length;
    $("export-hint").textContent = selected.length ? "正常使用会自动同步整份手机主库，无需复制。以下文本仅供备用核对；导出范围不影响同步范围。" : "这个范围没有可导出的记录，可以切换为全部清单。";
  }
  function openExport(ticket) {
    if (Object.keys(saving).length) { toast("正在保存出货状态，请稍后导出"); return; }
    exportSnapshot = rows.map(function (r) { return Object.assign({}, r, { issues:(r.issues || []).slice() }); });
    exportMeta = transport && transport.exportInfo ? transport.exportInfo() : { source:location.origin, cachedAt:Date.now() };
    exportTicket = ticket ? cur : "";
    $("export-scope").querySelector('[value="ticket"]').disabled = !ticket;
    $("export-scope").value = ticket ? "ticket" : "done";
    updateExport();
    $("export-sync").disabled=!transport;
    $("export-sync-info").textContent=transport?'出货、撤回自动保存；编辑需确认保存。连接电脑时自动同步，离线保留并重试。当前：'+transport.status():'当前为电脑网页。请在 iOS 出货助手中连接电脑进行主库同步。';
    $("export-panel").classList.remove("hidden");
    $("export-close").focus();
  }
  $("export-open").addEventListener("click", function () { openExport(false); });
  $("doc-export").addEventListener("click", function () { openExport(true); });
  $("export-close").addEventListener("click", function () {
    $("export-panel").classList.add("hidden");
    $(view === "doc" ? "doc-export" : "export-open").focus();
  });
  $("export-scope").addEventListener("change", updateExport);
  $("export-sync").addEventListener('click',async function(){
    if(!transport)return;
    var button=$("export-sync");button.disabled=true;button.textContent='正在同步…';
    $("export-sync-info").textContent='正在通过接口同步完整主库，不需要复制粘贴…';
    try{
      var data=await transport.sheet();applyRows(data.rows||[]);setSync(syncText());
      exportSnapshot=rows.map(function(r){return Object.assign({},r,{issues:(r.issues||[]).slice()});});
      exportMeta=transport.exportInfo?transport.exportInfo():exportMeta;updateExport();
      var conflicts=data.conflicts||[];
      $("export-sync-info").textContent=data.offline?'尚未同步到电脑。'+transport.status():conflicts.length?'已连接电脑，有 '+conflicts.length+' 项冲突，请关闭此窗口后进入同步详情核对。':'✓ 已同步到当前电脑，电脑已保存主库备份。';
      toast(data.offline?'手机数据已保留，电脑尚未同步':conflicts.length?'存在冲突，请核对':'已同步到当前电脑');
    }catch(error){$("export-sync-info").textContent='未同步成功：'+(error.message||'请检查电脑连接')+'。手机原数据保留。';}
    finally{button.disabled=false;button.textContent='同步到当前电脑';}
  });
  $("export-copy").addEventListener("click", async function () {
    var field = $("export-text");
    if (!field.value) return;
    try {
      if (!navigator.clipboard || !navigator.clipboard.writeText) throw new Error("manual");
      await navigator.clipboard.writeText(field.value);
      toast("已复制，可发送到电脑");
    } catch (_) {
      field.focus(); field.select(); field.setSelectionRange(0, field.value.length);
      var copied = false;
      try { copied = document.execCommand("copy"); } catch (_) {}
      if (copied) toast("已复制，可发送到电脑");
      else $("export-hint").textContent = "自动复制未成功。文本已选中，请长按文本选择“复制”，再发送到电脑。";
    }
  });

  function sumQty(list) {
    return list.reduce(function (n, r) {
      return n + (Number(r.qty) || 0);
    }, 0);
  }

  function nowText() {
    var d = new Date();
    return (
      "已同步 " +
      String(d.getHours()).padStart(2, "0") +
      ":" +
      String(d.getMinutes()).padStart(2, "0")
    );
  }

  /** 票的顺序按第一次出现；返回 [{waybill, rows}] */
  function tickets() {
    var map = {};
    var order = [];
    rows.forEach(function (r) {
      var w = r.waybill || "未识别单号";
      if (!map[w]) {
        map[w] = [];
        order.push(w);
      }
      map[w].push(r);
    });
    return order.map(function (w) {
      return { waybill: w, rows: map[w] };
    });
  }

  // 「入库号空」= 客户还没做箱唛、「数量 0」= 来了还没打包，都是正常状态，不算待确认
  var PROBLEM_ISSUES = { name: 1, inbound_sci: 1 };
  function hasProblem(r) {
    return !!(
      r.issues &&
      r.issues.some(function (c) {
        return PROBLEM_ISSUES[c];
      })
    );
  }

  function stats(list) {
    var total = sumQty(list);
    var done = sumQty(
      list.filter(function (r) {
        return r.checked;
      })
    );
    var doneRows = list.filter(function (r) {
      return r.checked;
    }).length;
    var issues = list.filter(hasProblem).length;
    return {
      total: total,
      done: done,
      left: total - done,
      n: list.length,
      doneRows: doneRows,
      issues: issues,
      pct: total ? Math.round((done / total) * 100) : 0,
      status: done === total && total > 0 ? "done" : doneRows > 0 ? "part" : "todo",
    };
  }

  function statusText(s) {
    // 用「出」不用「收」：这是出货清单，不是收货单（2026-11-01 用户要求）
    return s === "done" ? "已出" : s === "part" ? "部分" : "未出";
  }

  function renderCounts() {
    var all = stats(rows);
    $done.textContent = String(all.done);
    $left.textContent = String(all.left);
    var t = tickets();
    $sub.textContent = t.length + " 票 · " + all.n + " 行 · " + all.total + " 件";
  }

  /** 这一票的任务号（票内任意一行带 taskId 就行）；没有返回空 */
  function taskOf(t) {
    if (!t.waybill) return "";
    for (var i = 0; i < t.rows.length; i++) {
      if (t.rows[i].taskId) return t.rows[i].taskId;
    }
    return "";
  }

  /** 领取任务的那一行：没人领给按钮，有人领显示已领取 */
  function claimHtml(t) {
    if (!taskOf(t)) return "";
    var who = t.rows[0].claimedBy || "";
    if (who) {
      return '<div class="t-claim done">已领取 ✓</div>';
    }
    return (
      '<div class="t-claim"><button class="btn-claim" data-claim="' +
      esc(t.waybill) +
      '">领取任务</button></div>'
    );
  }

  function ticketHtml(t) {
    var s = stats(t.rows);
    var cust = t.rows[0].customer || "—";
    // tickets() 把空单号兜底成「未识别单号」，所以要看行本身的单号，不能看 t.waybill
    var noWb = !t.rows[0].waybill;
    return (
      '<article class="ticket ' +
      s.status +
      '" data-w="' +
      esc(t.waybill) +
      '">' +
      '<div class="t-top"><span class="t-no mono">' +
      (taskOf(t) ? '<b class="t-task">' + esc(taskOf(t)) + "</b> " : "") +
      esc(t.waybill) +
      (t.rows.some(function (r) { return (r.issues || []).some(function (c) { return c === "history_archive" || c === "waybill_archive"; }); }) ? ' <span class="tag archive">含存档</span>' : "") +
      '</span><span class="t-tag"><span class="tag ' +
      (s.status === "done" ? "ok" : s.status === "part" ? "part" : "") +
      '">' +
      statusText(s.status) +
      "</span></span></div>" +
      '<div class="t-meta' +
      (noWb ? " warn" : "") +
      '">' +
      (noWb
        ? s.n + " 行没有单号，补单号后才能出货"
        : esc(cust) + " · 共 " + s.total + " 件") +
      "</div>" +
      '<div class="t-bar"><i style="width:' +
      s.pct +
      '%"></i></div>' +
      '<div class="t-foot"><span>已出 ' +
      s.done +
      " 件</span><span>未出 " +
      s.left +
      " 件</span><span>" +
      s.pct +
      "%</span></div>" +
      claimHtml(t) +
      "</article>"
    );
  }

  function matchTicket(t) {
    var s = stats(t.rows);
    if (filter === "task" && !t.rows.some(function (r) { return r.task; })) return false;
    if (filter === "todo" && s.status === "done") return false;
    if (filter === "done" && s.status !== "done") return false;
    if (!q) return true;
    return t.rows.some(function (r) {
      return (
        (r.name + " " + r.inbound + " " + r.waybill + " " + r.customer)
          .toLowerCase()
          .indexOf(q) >= 0
      );
    });
  }

  // 搜索时按「票」归组：一个单号一个父节点，行挂在下面；没单号的并成最后一组
  function renderSearch() {
    var hits = rows.filter(function (r) {
      return (r.name + " " + r.inbound + " " + r.waybill + " " + r.customer)
        .toLowerCase()
        .indexOf(q) >= 0;
    });
    if (!hits.length) {
      $tickets.innerHTML =
        '<div class="empty">没有匹配的货<br/>换个单号 / 入库号 / 商品名试试</div>';
      return;
    }

    var groups = {};
    var order = [];
    hits.forEach(function (r) {
      var w = r.waybill || "未识别单号";
      if (!groups[w]) {
        groups[w] = [];
        order.push(w);
      }
      groups[w].push(r);
    });
    // 没单号的那堆永远排最后：它是「待补单号」，不是一票正常货
    order.sort(function (a, b) {
      var ua = a === "未识别单号";
      var ub = b === "未识别单号";
      if (ua !== ub) return ua ? 1 : -1;
      return 0;
    });

    $tickets.innerHTML =
      '<div class="hits">找到 ' +
      hits.length +
      " 行 · 归到 " +
      order.length +
      " 个组</div>" +
      order
        .map(function (w) {
          var list = groups[w];
          var s = stats(list);
          var noWb = w === "未识别单号";
          return (
            '<article class="hit" data-w="' +
            esc(w) +
            '">' +
            '<div class="hit-head"><span class="hit-w mono">' +
            esc(w) +
            '</span><span class="hit-cnt">' +
            list.length +
            " 行 · " +
            s.total +
            ' 件</span><span class="tag ' +
            (s.status === "done" ? "ok" : s.status === "part" ? "part" : "") +
            '">' +
            statusText(s.status) +
            "</span></div>" +
            (noWb ? '<div class="hit-tip">这批没有单号，补单号后才能出货</div>' : "") +
            '<div class="hit-kids">' +
            list
              .map(function (r) {
                return lineHtml(r);
              })
              .join("") +
            "</div></article>"
          );
        })
        .join("");
  }

  function renderTickets() {
    $tickets.classList.toggle("table-list", layout === "table");
    if (layout === "table") { renderTable(); return; }
    if (q) {
      renderSearch();
      return;
    }
    var list = tickets().filter(matchTicket);
    if (!list.length) {
      $tickets.innerHTML =
        '<div class="empty">' +
        (rows.length ? "这个筛选下没有票" : (transport ? "手机里还没有清单" : "电脑上还没打开表格")) +
        (transport ? "<br/>在顶部设置中配对电脑，首次同步后即可离线查看</div>" : "<br/>回电脑端选一个文件，这里会自动同步</div>");
      return;
    }
    $tickets.innerHTML = list.map(ticketHtml).join("");
  }

  function renderTable() {
    var shown = rows.filter(function (r) {
      if (filter === "todo" && r.checked || filter === "done" && !r.checked || filter === "task" && !r.task) return false;
      return !q || [r.customer, r.name, r.inbound, r.waybill, r.arrival || "", r.remarks || ""].join(" ").toLowerCase().indexOf(q) >= 0;
    });
    if (!shown.length) {
      $tickets.innerHTML = '<div class="empty">' + (rows.length ? '没有符合筛选的记录' : '手机还没有清单，连接电脑同步后可离线查看') + '</div>';
      return;
    }
    $tickets.innerHTML = '<div class="table-note">' + shown.length + ' 行 · ' + sumQty(shown) + ' 件</div>' + shown.map(lineHtml).join('');
  }

  $("layout-switch").addEventListener("click", function (e) {
    var button = e.target.closest("[data-layout]");
    if (!button) return;
    layout = button.getAttribute("data-layout");
    Array.prototype.forEach.call($("layout-switch").children, function (b) { b.classList.toggle("on", b === button); });
    renderTickets();
  });

  function specialLabel(r) {
    var issues = r.issues || [];
    return issues.indexOf("special_shenzhen") !== -1 ? "发深圳" : issues.indexOf("special_amazon") !== -1 ? "亚马逊" : "";
  }
  function isAmazon(r) { return !!specialLabel(r); }

  function lineHtml(r) {
    var k=esc(keyOf(r)), blocked=!r.checked&&r.blocked;
    return '<article class="row sheet-row'+(r.checked?' done':'')+(blocked?' blocked':'')+'" data-k="'+k+'">'+
      '<button class="tick" aria-pressed="'+r.checked+'" aria-label="'+(r.checked?'撤回出货':'标记已出')+'"'+(blocked||transport&&transport.waybillOnly?' disabled':'')+'>'+CHECK+'</button>'+
      '<div class="row-content"><div class="row-top"><div class="row-title"><span class="cust">'+esc(r.customer||'未填写客户')+'</span><span class="name">'+esc(r.name||'待补商品名')+'</span></div></div>'+
      '<div class="row-bottom"><div class="row-meta"><div class="qty"><span>数量</span><b>'+(r.quantityGroup&&(r.issues||[]).indexOf('qty_shared')>=0?'合箱':(Number(r.qty)||0))+'</b><span>'+(r.quantityGroup&&(r.issues||[]).indexOf('qty_shared')>=0?'':'件')+'</span></div><button class="inbound-copy" data-k="'+k+'" aria-label="复制入库号"><span>入库号</span><code>'+esc(r.inbound||'待补入库号')+'</code></button></div><button class="row-copy" data-k="'+k+'">▢ 复制</button><button class="row-edit" data-k="'+k+'" aria-label="编辑当前行">⋯</button></div>'+
      (specialLabel(r)?'<span class="tag row-special">'+esc(specialLabel(r))+'</span>':'')+(r.task?'<span class="tag task">任务</span>':'')+
      (hasProblem(r)?'<span class="tag">待核对</span>':'')+'</div></article>';
  }

  function rowCopyText(r) { return '客户：'+(r.customer||'')+'\n商品：'+(r.name||'')+'\n入库号：'+(r.inbound||'')+'\n数量：'+(Number(r.qty)||0)+'件'; }
  async function copyRowValue(text, button, message) {
    var original=button._copyOriginal||button.innerHTML;button._copyOriginal=original;clearTimeout(button._copyTimer);button.disabled=true;button.classList.remove('copied');
    if(button.classList.contains('row-copy'))button.textContent='复制中…';
    var copied=false;
    try { if (!navigator.clipboard) throw new Error('manual'); await navigator.clipboard.writeText(text); copied=true; }
    catch (_) { $("row-copy-text").value=text;$("row-copy-panel").classList.remove('hidden');$("row-copy-text").focus();$("row-copy-text").select();try{copied=document.execCommand('copy');}catch(_){}if(copied)$("row-copy-panel").classList.add('hidden'); }
    button.disabled=false;
    if(copied){button.classList.add('copied');if(button.classList.contains('row-copy'))button.textContent='✓ 已复制';toast(message);}
    else {button.innerHTML=original;toast('自动复制未成功，请长按文本复制');}
    button._copyTimer=setTimeout(function(){button.classList.remove('copied');button.innerHTML=original;},2200);
  }
  var editingRow=null;
  function rowAction(e) {
    var button=e.target.closest('.row-copy, .inbound-copy, .row-edit');if(!button)return false;
    e.stopPropagation();var k=button.getAttribute('data-k');var r=rows.find(function(item){return keyOf(item)===k;});if(!r)return true;
    if(button.classList.contains('row-copy'))void copyRowValue(rowCopyText(r),button,'已复制客户、商品、入库号和数量');
    else if(button.classList.contains('inbound-copy')){if(r.inbound)void copyRowValue(r.inbound,button,'入库号已复制');else toast('尚未填写入库号');}
    else {
      if(!transport||!transport.edit){toast('请在iOS手机主库中编辑，或在电脑端修改');return true;}
      editingRow=r;if(transport.waybillOnly||transport.ledgerOnly){$("row-edit-customer").disabled=true;$("row-edit-inbound").disabled=true;$("row-edit-special").disabled=true;}$("row-edit-customer").value=r.customer||'';$("row-edit-inbound").value=r.inbound||'';$("row-edit-waybill").value=r.waybill||'';$("row-edit-special").value=specialLabel(r);$("row-edit-error").textContent='';$("row-edit-panel").classList.remove('hidden');
    }
    return true;
  }
  $("row-copy-close").addEventListener('click',function(){$("row-copy-panel").classList.add('hidden');});
  $("row-edit-close").addEventListener('click',function(){$("row-edit-panel").classList.add('hidden');});
  $("row-edit-save").addEventListener('click',async function(){
    if(!editingRow)return;$("row-edit-save").disabled=true;
    try{
      var label=$("row-edit-special").value,waybill=$("row-edit-waybill").value.trim();
      var issues=(editingRow.issues||[]).filter(function(v){return v!=='special_amazon'&&v!=='special_shenzhen';});
      if(!transport.waybillOnly&&!transport.ledgerOnly){if(label){issues.push(label==='亚马逊'?'special_amazon':'special_shenzhen');waybill=label;}else if(waybill==='亚马逊'||waybill==='发深圳')waybill='';}
      await transport.edit(editingRow,(transport.waybillOnly||transport.ledgerOnly)?{waybill:waybill}:{customer:$("row-edit-customer").value.trim(),inbound:$("row-edit-inbound").value.trim(),waybill:waybill,issues:issues});
      applyRows(transport.cached());setSync(syncText());$("row-edit-panel").classList.add('hidden');toast('单号已保存在手机');
    }catch(error){$("row-edit-error").textContent=error.message||'保存失败';}finally{$("row-edit-save").disabled=false;}
  });
  var fieldNames={checked:'出货状态',waybill:'单号',customer:'客户',name:'商品',inbound:'入库号',destination:'收货仓',qty:'数量',arrival:'到货情况',remarks:'备注',cargo:'货物资料',issues:'特殊标注'};
  var cargoFieldNames={length:'长（cm）',width:'宽（cm）',height:'高（cm）',actualWeight:'实重（kg/箱）',sourceVolume:'体积/箱',remarks:'备注',perBox:'每箱数量',specification:'规格'};
  function conflictValue(value){
    if(value===null||value===undefined||value==='')return '未填写';
    if(typeof value==='boolean')return value?'已出货':'未出货';
    if(Array.isArray(value))return value.length?value.map(function(v){return v==='special_amazon'?'亚马逊':v==='special_shenzhen'?'发深圳':String(v);}).join('、'):'无';
    return String(value);
  }
  function conflictDetails(conflict){
    if(conflict.field==='cargo'){
      var phone=conflict.phone||{},computer=conflict.computer||{};
      var keys=Object.keys(Object.assign({},phone,computer)).filter(function(k){return JSON.stringify(phone[k])!==JSON.stringify(computer[k]);});
      return keys.map(function(k){return '<section class="sync-field-diff"><strong>'+esc(cargoFieldNames[k]||k)+'</strong><p>手机：'+esc(conflictValue(phone[k]))+'</p><p>电脑：'+esc(conflictValue(computer[k]))+'</p></section>';}).join('');
    }
    return '<p>手机：'+esc(conflictValue(conflict.phone))+'</p><p>电脑：'+esc(conflictValue(conflict.computer))+'</p>';
  }
  function resolutionLabel(conflict,side){
    var value=conflict[side],label=side==='phone'?'采用手机值':'采用电脑值';
    return conflict.field==='waybill'&&(value===null||value===undefined||value==='')?label+'（清空单号）':label;
  }
  function renderSyncPanel(){
    if(!transport||!transport.syncInfo){$("master-sync-info").textContent='当前为电脑网页。iOS应用支持随身主库及跨电脑同步。';return;}
    var info=transport.syncInfo();
    if(transport.waybillOnly||transport.ledgerOnly){
      var notes=info.waybillNotes||[],review=notes.filter(function(n){return n.needsMatch||n.cloudWaybill!==n.waybill;});
      $("master-sync-info").textContent=(transport.ledgerOnly?'云表只读；完整出货票据、单号和出货状态在手机与电脑双向同步。云表删号不会清除台账。\n当前电脑：':'电脑云表提供商品、数量和出货状态；手机单独记住单号。\n当前电脑：')+info.computer+'\n上次更新：'+(info.lastSync?new Date(info.lastSync).toLocaleString():'尚未更新')+'\n已记住 '+notes.length+' 个单号，'+review.length+' 项可核对。\n'+transport.status();
      $("master-conflicts").innerHTML=review.map(function(n){return '<article class="sync-conflict"><strong>'+esc(n.name)+'</strong><p>'+esc(n.inbound||'未填入库号')+'</p><p>手机记忆单号：<b>'+esc(n.waybill)+'</b></p>'+(n.needsMatch?'<p>云表明细已变化，单号仍保留，请选择对应的分批明细。</p>':'<p>云表单号：'+esc(n.cloudWaybill||'未填写')+'；手机记忆保留，不自动覆盖云表单号。</p>')+(n.needsMatch?(n.candidates.length?n.candidates.map(function(r){return '<button data-note="'+esc(n.id)+'" data-note-row="'+esc(r.syncId)+'">关联到'+(r.sourceRow?'原表第 '+esc(r.sourceRow)+' 行':'明细第 '+esc(r.seq)+' 条')+' · '+esc(r.qty)+' 箱 · '+(r.checked?'已出':'未出')+'</button>';}).join(''):'<p>当前云表暂无对应货物，单号继续保留在手机。</p>'):'')+'</article>';}).join('')||'<p>所有单号已保留，云表资料已更新；没有需要处理的资料冲突。</p>';
      if(transport.ledgerOnly&&info.conflicts.length){$("master-conflicts").innerHTML+=info.conflicts.map(function(c,i){return '<article class="sync-conflict"><strong>'+esc(c.name)+' · '+esc(fieldNames[c.field]||c.field)+'</strong>'+conflictDetails(c)+'<button data-conflict="'+i+'" data-choice="phone">采用手机值</button><button data-conflict="'+i+'" data-choice="computer">采用电脑值</button></article>';}).join('');}
      return;
    }
    $("master-sync-info").textContent='当前电脑：'+info.computer+'\n上次同步：'+(info.lastSync?new Date(info.lastSync).toLocaleString():'尚未同步')+'\n待同步：'+info.pending+' 条'+(info.otherPending?' · 另 '+info.otherPending+' 台电脑待确认':'')+'\n'+transport.status();
    $("master-conflicts").innerHTML=info.conflicts.map(function(c,i){return '<article class="sync-conflict"><strong>'+esc(c.name)+' · '+esc(fieldNames[c.field]||c.field)+'</strong>'+(c.inbound||c.sourceRow?'<p class="sync-conflict-source">'+esc(c.inbound||'未填入库号')+(c.sourceRow?' · 原表第 '+esc(c.sourceRow)+' 行':'')+'</p>':'')+conflictDetails(c)+'<div><button data-conflict="'+i+'" data-choice="phone">'+esc(resolutionLabel(c,'phone'))+'</button><button data-conflict="'+i+'" data-choice="computer">'+esc(resolutionLabel(c,'computer'))+'</button></div></article>';}).join('');
  }
  $("master-sync-open").addEventListener('click',function(){renderSyncPanel();$("master-sync-panel").classList.remove('hidden');});
  $("master-sync-close").addEventListener('click',function(){$("master-sync-panel").classList.add('hidden');});
  $("master-sync-now").addEventListener('click',async function(){
    $("master-sync-now").disabled=true;
    try{var data=await fetchSheet();applyRows(data.rows||[]);setSync(syncText());renderSyncPanel();toast(data.offline?'单号已保留，暂未连接电脑':'云表明细已更新，单号已保留');}catch(e){toast(e.message||'同步失败');}finally{$("master-sync-now").disabled=false;}
  });
  $("master-conflicts").addEventListener('click',async function(e){
    var noteButton=e.target.closest('[data-note]');
    if(noteButton&&transport&&transport.linkWaybill){noteButton.disabled=true;try{await transport.linkWaybill(noteButton.getAttribute('data-note'),noteButton.getAttribute('data-note-row'));applyRows(transport.cached());setSync(syncText());renderSyncPanel();}catch(error){toast(error.message||'关联失败');noteButton.disabled=false;}return;}
    var b=e.target.closest('[data-conflict]');if(!b||!transport||!transport.syncInfo)return;
    var conflict=transport.syncInfo().conflicts[Number(b.getAttribute('data-conflict'))];if(!conflict)return;b.disabled=true;
    try{await transport.resolveConflict(conflict.recordId,conflict.field,b.getAttribute('data-choice'));applyRows(transport.cached());setSync(syncText());renderSyncPanel();}catch(error){toast(error.message||'处理失败，请重试');b.disabled=false;}
  });

  function docRows() {
    return rows.filter(function (r) {
      return (r.waybill || "未识别单号") === cur;
    });
  }

  function docMatch(r) {
    if (!docQ) return true;
    return [r.waybill, r.inbound, r.name, r.customer]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .indexOf(docQ) >= 0;
  }

  function updateDocNumbers() {
    var list = docRows();
    var s = stats(list);
    $docBar.style.width = s.pct + "%";
    $docPct.textContent = s.doneRows + " / " + s.n + " 行";
    $docDone.textContent = String(s.done);
    $docLeft.textContent = String(s.left);
    $docDiff.textContent = "";
    $footDone.textContent = String(s.done);
    $footTotal.textContent = String(s.total);
    $docTag.textContent = statusText(s.status);
    $docTag.className =
      "tag " + (s.status === "done" ? "ok" : s.status === "part" ? "part" : "");
    var busySaving=receiving||Object.keys(saving).length>0;
    var canShip=docRows().some(function(r){return !r.checked&&!r.blocked&&(isAmazon(r)||(String(r.waybill||'').trim()&&String(r.inbound||'').trim()));});
    $receive.textContent = busySaving ? '正在保存…' : s.status === 'done' ? '✓ 本票已全部出货' : canShip ? '确认剩余 '+s.left+' 件出货' : '待补号码后出货';
    $receive.disabled=busySaving||s.status==='done'||!canShip;
    if(transport&&transport.waybillOnly){$receive.textContent='出货状态以电脑云文档为准';$receive.disabled=true;}
    $receive.classList.toggle('shipment-complete',s.status==='done'&&!busySaving);
  }

  function renderDoc() {
    var list = docRows();
    if (!list.length) {
      showList();
      return;
    }
    var s = stats(list);
    var custs = [];
    list.forEach(function (r) {
      if (r.customer && custs.indexOf(r.customer) < 0) custs.push(r.customer);
    });
    $docNo.textContent = cur;
    $docSub.textContent = (custs.join(" / ") || "—") + " · " + s.n + " 行";
    // 单据页也能搜：只影响显示的行，统计还是整张单
    var shown = list.filter(docMatch);
    $lines.innerHTML = shown.length
      ? shown.map(lineHtml).join("")
      : '<p style="margin:24px 0;text-align:center;color:var(--muted);font-size:13px">没有匹配的货</p>';
    updateDocNumbers();
  }

  function paintRow(r) {
    if (view === "list") { renderCounts(); renderTickets(); return; }
    var el = $lines.querySelector('[data-k="' + CSS.escape(keyOf(r)) + '"]');
    if (!el) return;
    el.classList.toggle("done", r.checked);
    var tick = el.querySelector(".tick");
    if (tick) tick.setAttribute("aria-pressed", String(r.checked));
  }

  /** 告诉外壳现在是列表还是单据页：单据页底部有「确认出货」，右下角齿轮要躲开 */
  function tellShell(name) {
    if (!window.parent || window.parent === window) return;
    try {
      window.parent.postMessage({ source: "shipping-mobile", type: "view", view: name }, "*");
    } catch (e) {}
  }

  function showDoc(w) {
    cur = w;
    view = "doc";
    tellShell("doc");
    $viewList.classList.add("hidden");
    $viewDoc.classList.remove("hidden");
    window.scrollTo(0, 0);
    $lines.scrollTop = 0;
    $dq.value = "";
    docQ = "";
    renderDoc();
  }

  function showList() {
    cur = "";
    view = "list";
    tellShell("list");
    $viewDoc.classList.add("hidden");
    $viewList.classList.remove("hidden");
    window.scrollTo(0, 0);
    renderCounts();
    renderTickets();
  }

  function fetchSheet() {
    if (transport) return transport.sheet();
    return apiFetch("/api/sheet", { cache: "no-store" }).then(function (res) {
      if (res.status === 403) throw new Error("forbidden");
      if (!res.ok) throw new Error("http " + res.status);
      return res.json();
    });
  }

  function applyRows(next) {
    var sameShape =
      rows.length === next.length &&
      rows.every(function (r, i) {
        var n = next[i];
        return keyOf(r) === keyOf(n) && r.customer === n.customer && r.qty === n.qty && r.arrival === n.arrival && r.remarks === n.remarks && r.blocked === n.blocked && r.task === n.task && JSON.stringify(r.issues) === JSON.stringify(n.issues);
      });
    rows = next;
    renderCounts();
    if (view === "doc") {
      if (!docRows().length) {
        showList();
        return;
      }
      if (sameShape) {
        docRows().forEach(paintRow);
        updateDocNumbers();
      } else {
        renderDoc();
      }
    } else {
      renderTickets();
    }
  }

  function load(first) {
    setSync(first ? "连接中…" : "刷新中…");
    fetchSheet()
      .then(function (data) {
        applyRows(data.rows || []);
        setSync(syncText());
      })
      .catch(function (e) {
        setSync(
          e.message === "forbidden" ? "链接失效，回电脑重新扫码" : (transport ? e.message : "连接断开 · 点右上角刷新"),
          true
        );
      });
  }

  function postCheck(r, checked) {
    if (transport) return transport.check(r, checked).then(function (result) { if (!result.offline) r.version += 1; });
    return apiFetch("/api/check", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        waybill: r.waybill,
        inbound: r.inbound,
        name: r.name,
        sourceId: r.sourceId, recordId: r.recordId, datasetId: r.datasetId, version: r.version,
        checked: checked,
      }),
    }).then(function (res) {
      if (!res.ok) return res.json().then(function (data) { throw new Error(data.error || "保存失败"); });
      r.version += 1;
    });
  }

  function toggle(k) {
    if(transport&&transport.waybillOnly){toast("出货状态以电脑云文档为准");return;}
    if (cloudRefreshing) { toast("正在获取云端最新清单，请稍后操作"); return; }
    var r = null;
    for (var i = 0; i < rows.length; i++) {
      if (keyOf(rows[i]) === k) {
        r = rows[i];
        break;
      }
    }
    if (!r || saving[k]) return;
    if (!r.checked && !isAmazon(r) && (!String(r.waybill || "").trim() || !String(r.inbound || "").trim())) { toast("入库号和单号必须都填写，才能出货"); return; }
    var next = !r.checked;
    r.checked = next;
    saving[k] = true;
    paintRow(r);
    updateDocNumbers();
    setSync("保存中…");

    postCheck(r, next)
      .then(function () {
        setSync(syncText());
      })
      .catch(function (error) {
        r.checked = !next;
        paintRow(r);
        updateDocNumbers();
        toast(error.message || "没保存上，请刷新核对后重试");
        load(false);
        setSync("连接断开 · 点右上角刷新", true);
      })
      .finally(function () {
        delete saving[k];
        updateDocNumbers();
      });
  }

  function receive() {
    if(receiving||Object.keys(saving).length)return;
    var list = docRows();
    var pending = list.filter(function (r) {
      return !r.checked && !r.blocked && (isAmazon(r) || (String(r.waybill || "").trim() && String(r.inbound || "").trim())); // 没入库号/单号的跳过，勾不动
    });
    if(!pending.length){updateDocNumbers();return;}
    if (!confirm("还有 " + pending.length + " 行没勾，确认全部标记为已出？")) {
      return;
    }
    receiving=true;
    updateDocNumbers();
    setSync("提交中…");
    var chain = Promise.resolve();
    pending.forEach(function (r) {
      chain = chain.then(function () {
        r.checked = true;
        paintRow(r);
        return postCheck(r, true);
      });
    });
    chain
      .then(function () {
        updateDocNumbers();
        toast(transport && transport.status() ? transport.status() : "本票已确认出货");
        setSync(syncText());
        setTimeout(showList, 450);
      })
      .catch(function () {
        toast("有一行未保存，请刷新核对后重试");
        load(false);
        setSync("连接断开 · 点右上角刷新", true);
        updateDocNumbers();
      })
      .finally(function () {
        receiving=false;
        updateDocNumbers();
      });
  }

  function claimTask(waybill) {
    var who = "我";
    setSync("领取中…");
    apiFetch("/api/claim", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ waybill: waybill, who: who }),
    })
      .then(function (r) {
        return r.json();
      })
      .then(function (j) {
        if (j.error) {
          toast(j.error);
          return;
        }
        var owner = j.claimedBy || who;
        toast(owner === who ? "已领取，去找货吧" : "这票已经被 " + owner + " 领了");
        // 立刻回填，别等下一轮轮询
        rows.forEach(function (r) {
          if (r.waybill === waybill) r.claimedBy = owner;
        });
        renderTickets();
        setSync(syncText());
      })
      .catch(function () {
        toast("领取失败，检查网络");
        setSync("连接断开 · 点右上角刷新", true);
      });
  }

  $tickets.addEventListener("click", function (e) {
    if (rowAction(e)) return;
    var openWaybill = e.target.closest("[data-open-w]");
    if (openWaybill) { showDoc(openWaybill.getAttribute("data-open-w")); return; }
    var claimBtn = e.target.closest("[data-claim]");
    if (claimBtn) {
      e.stopPropagation();
      claimTask(claimBtn.getAttribute("data-claim"));
      return;
    }
    // 搜索结果里：点勾直接标已出，点别处进这张单
    var tick = e.target.closest(".hit .tick, .sheet-row .tick");
    if (tick) {
      e.stopPropagation();
      var rowEl = tick.closest(".row");
      if (rowEl && rowEl.classList.contains("blocked")) {
        toast("入库号和单号必须都填写，才能出货");
        return;
      }
      if (rowEl) toggle(rowEl.getAttribute("data-k"));
      return;
    }
    var hit = e.target.closest(".hit");
    if (hit) {
      showDoc(hit.getAttribute("data-w"));
      return;
    }
    var card = e.target.closest(".ticket");
    if (!card) return;
    showDoc(card.getAttribute("data-w"));
  });

  $lines.addEventListener("click", function (e) {
    if (rowAction(e)) return;
    if (!e.target.closest(".tick")) return;
    var row = e.target.closest(".row");
    if (!row) return;
    // 没入库号也没单号的货不能出货：点了只提示，不改状态
    if (row.classList.contains("blocked")) {
      toast("入库号和单号必须都填写，才能出货");
      return;
    }
    toggle(row.getAttribute("data-k"));
  });

  $seg.addEventListener("click", function (e) {
    var btn = e.target.closest("button");
    if (!btn) return;
    filter = btn.getAttribute("data-f");
    Array.prototype.forEach.call($seg.children, function (b) {
      b.classList.toggle("on", b === btn);
    });
    renderTickets();
  });

  var qTimer = null;
  $dq.addEventListener("input", function () {
    docQ = $dq.value.trim().toLowerCase();
    renderDoc();
  });

  $q.addEventListener("input", function () {
    clearTimeout(qTimer);
    qTimer = setTimeout(function () {
      q = $q.value.trim().toLowerCase();
      renderTickets();
    }, 120);
  });

  $("back").addEventListener("click", showList);
  $("refresh").addEventListener("click", async function () {
    if (cloudRefreshing) return;
    if (Object.keys(saving).length) { toast("请等待出货状态保存后再刷新"); return; }
    cloudRefreshing = true;
    $("refresh").disabled = true;
    setSync("正在通知电脑获取云表格…");
    try {
      if (transport && transport.prepareRefresh) await transport.prepareRefresh();
      var response = await apiFetch("/api/refresh", {method:"POST"});
      var task = await response.json().catch(function () { return {}; });
      if (!response.ok) throw new Error(response.status === 404 ? "请先更新电脑端，当前版本不支持云刷新" : task.error || "无法通知电脑刷新");
      var id = task.id, deadline = Date.now() + 150000;
      while (task.state !== "done" && task.state !== "error") {
        if (Date.now() > deadline) throw new Error("电脑获取云表格超时，请检查电脑连接和WPS登录状态");
        setSync(task.state === "running" ? "电脑正在检查云表格，请稍候…" : "等待电脑处理刷新请求…");
        await new Promise(function (resolve) { setTimeout(resolve, 1000); });
        var res = await apiFetch("/api/refresh", {cache:"no-store"});
        if (!res.ok) throw new Error("无法读取电脑刷新进度");
        task = await res.json();
        if (task.id !== id) throw new Error("电脑连接发生变化，请重新刷新");
      }
      if (task.state === "error") throw new Error(task.error || "云表格刷新失败");
      var data = await fetchSheet();
      if (data.offline) throw new Error("电脑已刷新，但手机未能读取最新清单，请重试");
      applyRows(data.rows || []);
      setSync("云表格已刷新 · " + nowText());
      toast("已获取云表格最新清单");
    } catch (error) {
      setSync(error.message || "刷新失败，当前保留原清单", true);
      toast(error.message || "刷新失败，当前保留原清单");
    } finally { cloudRefreshing = false; $("refresh").disabled = false; }
  });
  $receive.addEventListener("click", receive);

  pollTimer = setInterval(function () {
    if (cloudRefreshing) return;
    if (Object.keys(saving).length) return;
    if (document.hidden) return;
    fetchSheet()
      .then(function (data) {
        applyRows(data.rows || []);
        setSync(syncText());
      })
      .catch(function (e) { setSync(e.message || "同步失败", true); });
  }, 8000);

  window.addEventListener("visibilitychange", function () {
    if (!document.hidden && !cloudRefreshing) load(false);
  });

  if (transport) { applyRows(transport.cached()); setSync("本地清单 · 正在检查连接"); }
  load(true);
  if (window.parent !== window) window.parent.postMessage({source:'shipping-mobile', type:'ui-ready'}, '*');
})();
