import { Button } from "antd";

type Props = {
  saving: boolean;
  failure: { message: string } | null;
  error: string | null;
  onRetry: () => void;
  onDiscard: () => void;
};

export function PackingSaveStatus({ saving, failure, error, onRetry, onDiscard }: Props) {
  return <>
    {saving && <div className="banner" role="status">正在保存出货状态，请稍候…</div>}
    {failure && <div className="banner err" role="alert">
      本次出货操作未保存：{failure.message}
      <Button disabled={saving} onClick={onRetry}>重试保存</Button>
      <Button disabled={saving} onClick={onDiscard}>放弃本次操作</Button>
    </div>}
    {error && <div className="banner err">❌ {error}</div>}
  </>;
}
