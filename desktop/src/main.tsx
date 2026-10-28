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
          colorPrimary: "#5c7fa3",
          colorInfo: "#5c7fa3",
          colorSuccess: "#4f7d63",
          colorWarning: "#a67c52",
          colorError: "#b3564f",
          colorBgLayout: "#f3f1ed",
          colorBgContainer: "#fffefb",
          colorBorderSecondary: "#e5e1d9",
          colorText: "#2f3339",
          colorTextSecondary: "#767268",
          colorTextTertiary: "#8d897f",
          borderRadius: 8,
          fontSize: 14,
        },
        components: {
          Table: {
            headerBg: "#f2efe9",
            headerColor: "#6a665d",
            rowHoverBg: "#f6f4ef",
          },
          Card: {
            headerBg: "transparent",
          },
        },
      }}
    >
      <App />
    </ConfigProvider>
  </React.StrictMode>,
);
