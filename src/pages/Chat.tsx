/** 聊天页：MVP 之后再接，先保留入口与说明。 */
export function Chat() {
  return (
    <div className="flex h-full items-center justify-center p-4">
      <div className="max-w-sm text-center">
        <h2 className="m-0 mb-2 text-[15px] font-medium">对话页</h2>
        <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
          这里会是常规聊天框，可选择「按需」或「常驻」两种驻留模式。
          <br />
          MVP 阶段先聚焦翻译页闭环。
        </p>
      </div>
    </div>
  );
}
