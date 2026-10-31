import React from "react";
import ReactDOM from "react-dom/client";
import { ConfigProvider } from "antd";
import zhCN from "antd/locale/zh_CN";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ConfigProvider
      locale={zhCN}
      theme={{
        token: {
          // 与 dsh-context 同一套 DSH 令牌：主色近黑，蓝色只用于信息与数据
          colorPrimary: "#0f1115",
          colorPrimaryHover: "#43454a",
          colorPrimaryActive: "#1b1b1c",
          colorInfo: "#3b82f6",
          colorSuccess: "#22c55e",
          colorWarning: "#a16207",
          colorError: "#d33b3b",
          colorBgLayout: "#f1f3f5",
          colorBgContainer: "#ffffff",
          colorBgElevated: "#ffffff",
          colorBorder: "#e1e5ee",
          colorBorderSecondary: "#eef0f2",
          colorText: "#0f1115",
          colorTextSecondary: "#61666b",
          colorTextTertiary: "#81858c",
          colorTextQuaternary: "#adb2b8",
          colorSplit: "#eef0f2",
          borderRadius: 6,
          borderRadiusLG: 10,
          borderRadiusSM: 4,
          controlHeight: 30,
          fontSize: 13,
          fontFamily:
            '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif',
          boxShadowTertiary: "0 2px 4px rgba(0, 0, 0, 0.05), 0 4px 12px rgba(0, 0, 0, 0.02)",
          wireframe: false,
        },
        components: {
          Button: {
            primaryShadow: "0 1px 2px rgba(0, 0, 0, 0.12)",
            defaultBg: "#ffffff",
            defaultBorderColor: "#d9dde3",
            defaultColor: "#43454a",
            fontWeight: 500,
          },
          Table: {
            headerBg: "#f9fafb",
            headerColor: "#61666b",
            headerSplitColor: "transparent",
            rowHoverBg: "#f5f6f7",
            borderColor: "#eef0f2",
            cellPaddingBlock: 8,
          },
          Card: {
            headerBg: "transparent",
            colorBorderSecondary: "#eef0f2",
          },
          Input: { activeBorderColor: "#0f1115", hoverBorderColor: "rgba(15,17,21,0.3)" },
          Select: { optionSelectedBg: "#f1f3f5" },
          Checkbox: { borderRadiusSM: 4 },
          Modal: { borderRadiusLG: 14 },
          Drawer: { colorBgElevated: "#ffffff" },
        },
      }}
    >
      <App />
    </ConfigProvider>
  </React.StrictMode>,
);
