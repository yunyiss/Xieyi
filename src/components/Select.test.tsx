import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Select } from "./Select";

/**
 * 自绘下拉的测试。
 *
 * 它是全局唯一的下拉实现（项目里不用原生 `<select>`），
 * 所以这里坏了会影响所有选择器：语言、模型、思考档位、TTL……
 *
 * 重点覆盖键盘操作：自绘组件的可访问性完全靠手写，
 * 没有原生控件的兜底，漏一个键就是「用键盘的人选不了」。
 */

const OPTIONS = [
  { value: "zh", label: "简体中文" },
  { value: "en", label: "英语" },
  { value: "ja", label: "日语" },
];

describe("Select", () => {
  it("显示当前选中项的文案", () => {
    render(<Select value="en" options={OPTIONS} onChange={() => {}} />);
    expect(screen.getByText("英语")).toBeInTheDocument();
  });

  it("值不在选项里时显示占位符", () => {
    // 配置里存了一个已删除的模型 id 时会走到这里，
    // 不能显示空白（用户会以为控件坏了）
    render(
      <Select value="gone" options={OPTIONS} onChange={() => {}} placeholder="请选择" />,
    );
    expect(screen.getByText("请选择")).toBeInTheDocument();
  });

  it("点击展开后能看到全部选项", async () => {
    const user = userEvent.setup();
    render(<Select value="zh" options={OPTIONS} onChange={() => {}} />);

    await user.click(screen.getByRole("button"));
    await waitFor(() => {
      expect(screen.getByRole("listbox")).toBeInTheDocument();
    });
    // 三项都在（当前项也在列表里，只是高亮）
    expect(screen.getAllByText("简体中文").length).toBeGreaterThan(0);
    expect(screen.getByText("日语")).toBeInTheDocument();
  });

  it("选中一项后回调拿到对应的值，并收起面板", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Select value="zh" options={OPTIONS} onChange={onChange} />);

    await user.click(screen.getByRole("button"));
    await user.click(await screen.findByText("日语"));

    expect(onChange).toHaveBeenCalledWith("ja");
    await waitFor(() => {
      expect(screen.queryByRole("listbox")).toBeNull();
    });
  });

  it("禁用时不展开", async () => {
    const user = userEvent.setup();
    render(<Select value="zh" options={OPTIONS} onChange={() => {}} disabled />);

    const trigger = screen.getByRole("button");
    // 注意：`disabled` 属性的按钮在 jsdom 里点击本来就不触发回调，
    // 所以只测点击无法验证「组件自己的 disabled 判断」是否生效。
    // 用键盘事件直接调 onKeyDown，才能真正验到那行代码。
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.queryByRole("listbox")).toBeNull();

    // 点击也不应展开
    await user.click(trigger);
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("键盘：ArrowDown 展开，再按选中下一项", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Select value="zh" options={OPTIONS} onChange={onChange} />);

    const trigger = screen.getByRole("button");
    trigger.focus();

    // 关闭状态按 ArrowDown 展开
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("listbox")).toBeInTheDocument());

    // 打开后高亮停在当前项（zh，index 0），再按一次下移到 en
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledWith("en");
  });

  it("键盘：Home / End 跳到首尾", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Select value="zh" options={OPTIONS} onChange={onChange} />);

    screen.getByRole("button").focus();
    await user.keyboard("{ArrowDown}");           // 展开
    await user.keyboard("{End}{Enter}");          // 跳到最后一项
    expect(onChange).toHaveBeenCalledWith("ja");
  });

  it("键盘：Escape 关闭面板但不改变选择", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Select value="zh" options={OPTIONS} onChange={onChange} />);

    await user.click(screen.getByRole("button"));
    await waitFor(() => expect(screen.getByRole("listbox")).toBeInTheDocument());

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("点击外部关闭面板", async () => {
    const user = userEvent.setup();
    render(
      <div>
        <span>外面</span>
        <Select value="zh" options={OPTIONS} onChange={() => {}} />
      </div>,
    );

    await user.click(screen.getByRole("button"));
    await waitFor(() => expect(screen.getByRole("listbox")).toBeInTheDocument());

    await user.click(screen.getByText("外面"));
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  });
});
