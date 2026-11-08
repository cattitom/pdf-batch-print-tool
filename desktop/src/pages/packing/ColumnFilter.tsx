import { useEffect, useRef, useState } from "react";
import { Button, Checkbox, Input, Popover } from "antd";
import { CloseOutlined, FilterOutlined, SearchOutlined } from "@ant-design/icons";

export type FilterOption = { text: string; value: string; count?: number };

/** 滚动表格会复制表头测量宽度；每个真实触发器独立管理弹层，避免副本互相关闭。 */
export function ColumnFilterTitle({ label, options, value, onApply }: {
  label: string; options: FilterOption[]; value: string[] | null;
  onApply: (value: string[] | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [session, setSession] = useState(0);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Element && !trigger.current?.contains(target) && !target.closest(".packing-filter-popover")) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);
  const active = !!value?.length;
  return <Popover trigger={[]} placement="bottomLeft" arrow={false}
    open={open} onOpenChange={setOpen} classNames={{ root: "packing-filter-popover" }}
    styles={{ container: { padding: 0, borderRadius: 10, overflow: "hidden" }, content: { padding: 0 } }}
    content={open ? <ColumnFilter key={session} label={label} options={options} value={value}
      onApply={onApply} onClose={() => setOpen(false)} /> : null}>
    <button ref={trigger} type="button" onClick={() => { if (!open) setSession((value) => value + 1); setOpen(!open); }}
      className={`packing-column-filter${active ? " active" : ""}`}
      aria-label={`筛选${label}`} aria-expanded={open} aria-haspopup="dialog">
      <span>{label}</span><FilterOutlined />{active && <i className="packing-filter-dot" />}
    </button>
  </Popover>;
}

/** 编辑草稿后统一应用；关闭弹层不改变当前表格。 */
export function ColumnFilter({ label, options, value, onApply, onClose }: {
  label: string;
  options: FilterOption[];
  value: string[] | null;
  onApply: (value: string[] | null) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<string[]>(value ? [...value] : []);
  const shown = options.filter((o) => o.text.toLowerCase().includes(query.trim().toLowerCase()));
  return (
    <section className="packing-filter-panel" role="dialog" aria-label={`筛选${label}`}
      onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
      <header className="packing-filter-heading">
        <span>{label}</span>
        <button className="packing-filter-close" aria-label="关闭筛选" onClick={onClose}><CloseOutlined /></button>
      </header>
      {options.length > 8 && <div className="packing-filter-search">
        <Input autoFocus allowClear prefix={<SearchOutlined />} placeholder="搜索选项"
          aria-label={`搜索${label}选项`} value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>}
      <div className="packing-filter-options">
        {shown.map((o) => <Checkbox key={o.value} className={`packing-filter-option${draft.includes(o.value) ? " selected" : ""}`}
          checked={draft.includes(o.value)} onChange={(e) => setDraft((prev) =>
            e.target.checked ? [...prev, o.value] : prev.filter((v) => v !== o.value))}>
          <span className="packing-filter-option-text">{o.text}</span>
          {o.count !== undefined && <span className="packing-filter-option-count">{o.count} 条</span>}
        </Checkbox>)}
        {!shown.length && <div className="packing-filter-empty">没有匹配的选项</div>}
      </div>
      <footer className="packing-filter-footer">
        <Button type="text" size="small" onClick={() => { setDraft([]); setQuery(""); }}>重置</Button>
        <Button type="primary" size="small" onClick={() => { onApply(draft.length ? draft : null); onClose(); }}>应用筛选</Button>
      </footer>
    </section>
  );
}
