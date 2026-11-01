/* 手机出货（A 方案）：票列表 → 单据页（单据头 + 行项目 + 确认出货）。无框架。 */
(function () {
  "use strict";

  var TOKEN = new URLSearchParams(location.search).get("t") || "";
  var CHECK =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 13l4 4L19 7"/></svg>';

  var rows = [];
  var view = "list";
  var cur = "";
  var filter = "all";
  var q = "";
  var docQ = "";
  var saving = {};
  var pollTimer = null;

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
    return r.waybill + "|" + r.inbound + "|" + r.name;
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
    return (
      '<article class="ticket ' +
      s.status +
      '" data-w="' +
      esc(t.waybill) +
      '">' +
      '<div class="t-top"><span class="t-no mono">' +
      (taskOf(t) ? '<b class="t-task">' + esc(taskOf(t)) + "</b> " : "") +
      esc(t.waybill) +
      '</span><span class="t-tag"><span class="tag ' +
      (s.status === "done" ? "ok" : s.status === "part" ? "part" : "") +
      '">' +
      statusText(s.status) +
      "</span></span></div>" +
      '<div class="t-meta">' +
      esc(cust) +
      " · 共 " +
      s.total +
      " 件</div>" +
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

  function renderTickets() {
    var list = tickets().filter(matchTicket);
    if (!list.length) {
      $tickets.innerHTML =
        '<div class="empty">' +
        (rows.length ? "这个筛选下没有票" : "电脑上还没打开表格") +
        "<br/>回电脑端选一个文件，这里会自动同步</div>";
      return;
    }
    $tickets.innerHTML = list.map(ticketHtml).join("");
  }

  function lineHtml(r) {
    return (
      '<article class="row' +
      (r.checked ? " done" : "") +
      (!r.checked && r.blocked ? " blocked" : "") +
      '" data-k="' +
      esc(keyOf(r)) +
      '">' +
      '<button class="tick" aria-pressed="' +
      r.checked +
      '" aria-label="标记已出"' +
      (!r.checked && r.blocked ? ' disabled title="没有入库号也没有单号，不能出货"' : "") +
      ">" +
      CHECK +
      "</button>" +
      '<div class="body">' +
      // 顺序：客户 → 商品 → 入库号（件数在最右，2026-11-01 用户要求）
      '<span class="cust" title="' + esc(r.customer || "") + '">' +
      esc(r.customer || "—") +
      "</span>" +
      '<div class="name">' +
      esc(r.name) +
      "</div>" +
      '<span class="mono inb">' +
      esc(r.inbound || "无入库号") +
      "</span>" +
      (r.task ? '<span class="tag task">任务</span>' : "") +
      (hasProblem(r) ? '<span class="tag">待确认</span>' : "") +
      "</div>" +
      '<div class="qty"><b>' +
      (Number(r.qty) || 0) +
      "</b><span>件</span></div>" +
      "</article>"
    );
  }

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
    $receive.textContent = s.status === "done" ? "确认出货" : "确认出货（还差 " + s.left + " 件）";
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
    var el = $lines.querySelector('[data-k="' + CSS.escape(keyOf(r)) + '"]');
    if (!el) return;
    el.classList.toggle("done", r.checked);
    var tick = el.querySelector(".tick");
    if (tick) tick.setAttribute("aria-pressed", String(r.checked));
  }

  function showDoc(w) {
    cur = w;
    view = "doc";
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
    $viewDoc.classList.add("hidden");
    $viewList.classList.remove("hidden");
    window.scrollTo(0, 0);
    renderCounts();
    renderTickets();
  }

  function fetchSheet() {
    return fetch(url("/api/sheet"), { cache: "no-store" }).then(function (res) {
      if (res.status === 403) throw new Error("forbidden");
      if (!res.ok) throw new Error("http " + res.status);
      return res.json();
    });
  }

  function applyRows(next) {
    var sameShape =
      rows.length === next.length &&
      rows.every(function (r, i) {
        return keyOf(r) === keyOf(next[i]);
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
        setSync(nowText());
      })
      .catch(function (e) {
        setSync(
          e.message === "forbidden" ? "链接失效，回电脑重新扫码" : "连接断开 · 点右上角刷新",
          true
        );
      });
  }

  function postCheck(r, checked) {
    return fetch(url("/api/check"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        waybill: r.waybill,
        inbound: r.inbound,
        name: r.name,
        checked: checked,
      }),
    }).then(function (res) {
      if (!res.ok) throw new Error("http " + res.status);
    });
  }

  function toggle(k) {
    var r = null;
    for (var i = 0; i < rows.length; i++) {
      if (keyOf(rows[i]) === k) {
        r = rows[i];
        break;
      }
    }
    if (!r || saving[k]) return;
    var next = !r.checked;
    r.checked = next;
    saving[k] = true;
    paintRow(r);
    updateDocNumbers();
    setSync("保存中…");

    postCheck(r, next)
      .then(function () {
        setSync(nowText());
      })
      .catch(function () {
        r.checked = !next;
        paintRow(r);
        updateDocNumbers();
        toast("没保存上，检查一下是不是断了");
        setSync("连接断开 · 点右上角刷新", true);
      })
      .finally(function () {
        delete saving[k];
      });
  }

  function receive() {
    var list = docRows();
    var pending = list.filter(function (r) {
      return !r.checked && !r.blocked; // 没入库号/单号的跳过，勾不动
    });
    if (pending.length && !confirm("还有 " + pending.length + " 行没勾，确认全部标记为已出？")) {
      return;
    }
    $receive.disabled = true;
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
        toast("本票已确认出货");
        setSync(nowText());
        setTimeout(showList, 450);
      })
      .catch(function () {
        toast("有一行没保存上，再试一次");
        setSync("连接断开 · 点右上角刷新", true);
        updateDocNumbers();
      })
      .finally(function () {
        $receive.disabled = false;
      });
  }

  function claimTask(waybill) {
    var who = "我";
    setSync("领取中…");
    fetch(url("/api/claim"), {
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
        setSync(nowText());
      })
      .catch(function () {
        toast("领取失败，检查网络");
        setSync("连接断开 · 点右上角刷新", true);
      });
  }

  $tickets.addEventListener("click", function (e) {
    var claimBtn = e.target.closest("[data-claim]");
    if (claimBtn) {
      e.stopPropagation();
      claimTask(claimBtn.getAttribute("data-claim"));
      return;
    }
    var card = e.target.closest(".ticket");
    if (!card) return;
    showDoc(card.getAttribute("data-w"));
  });

  $lines.addEventListener("click", function (e) {
    var row = e.target.closest(".row");
    if (!row) return;
    // 没入库号也没单号的货不能出货：点了只提示，不改状态
    if (row.classList.contains("blocked")) {
      toast("没有入库号也没有单号，不能出货");
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
  $("refresh").addEventListener("click", function () {
    load(false);
  });
  $receive.addEventListener("click", receive);

  pollTimer = setInterval(function () {
    if (Object.keys(saving).length) return;
    if (document.hidden) return;
    fetchSheet()
      .then(function (data) {
        applyRows(data.rows || []);
      })
      .catch(function () {});
  }, 8000);

  window.addEventListener("visibilitychange", function () {
    if (!document.hidden) load(false);
  });

  load(true);
})();


