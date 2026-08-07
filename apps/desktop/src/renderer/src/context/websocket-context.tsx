/* eslint-disable react/jsx-no-constructed-context-values */
import React, { useContext } from "react";
import { wsService } from "@/services/websocket-service";

const DEFAULT_WS_URL = "ws://127.0.0.1:12394/client-ws";
const DEFAULT_BASE_URL = "http://127.0.0.1:12394";

export interface HistoryInfo {
  uid: string;
  latest_message: {
    role: "human" | "ai";
    timestamp: string;
    content: string;
  } | null;
  timestamp: string | null;
}

interface WebSocketContextProps {
  sendMessage: (message: object) => void;
  sendBinary: (data: ArrayBuffer | ArrayBufferView) => void;
  wsState: string;
  reconnect: () => void;
  wsUrl: string;
  setWsUrl: (url: string) => void;
  baseUrl: string;
  setBaseUrl: (url: string) => void;
}

export const WebSocketContext = React.createContext<WebSocketContextProps>({
  sendMessage: wsService.sendMessage.bind(wsService),
  sendBinary: wsService.sendBinary.bind(wsService),
  wsState: "CLOSED",
  reconnect: () => wsService.connect(DEFAULT_WS_URL),
  wsUrl: DEFAULT_WS_URL,
  setWsUrl: () => {},
  baseUrl: DEFAULT_BASE_URL,
  setBaseUrl: () => {},
});

export function useWebSocket() {
  const context = useContext(WebSocketContext);
  if (!context) {
    throw new Error("useWebSocket must be used within a WebSocketProvider");
  }
  return context;
}

export const defaultWsUrl = DEFAULT_WS_URL;
export const defaultBaseUrl = DEFAULT_BASE_URL;
