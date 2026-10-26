import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// 没开 vitest 的 globals，Testing Library 不会自动清理，这里手动收尾
afterEach(() => cleanup());
