type MessageHandler = (data: unknown) => void;

export class WebSocketManager {
  private ws: WebSocket | null = null;
  private url: string = '';
  private subscriptions = new Map<string, Set<MessageHandler>>();
  private reconnectAttempts = 0;
  private maxReconnects = 10;
  private reconnectDelay = 1000;
  private pingInterval: ReturnType<typeof setInterval> | null = null;

  connect(url: string): void {
    this.url = url;
    this.ws = new WebSocket(url);
    this.ws.onopen = () => {
      this.reconnectAttempts = 0;
      this.startPing();
      // Re-subscribe all streams
      const streams = Array.from(this.subscriptions.keys());
      if (streams.length > 0) {
        this.send({ method: 'SUBSCRIBE', params: streams, id: Date.now() });
      }
    };
    this.ws.onmessage = (event) => this.handleMessage(event);
    this.ws.onclose = () => {
      this.stopPing();
      this.reconnect();
    };
    this.ws.onerror = () => {};
  }

  subscribe(stream: string, callback: MessageHandler): () => void {
    if (!this.subscriptions.has(stream)) {
      this.subscriptions.set(stream, new Set());
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.send({ method: 'SUBSCRIBE', params: [stream], id: Date.now() });
      }
    }
    this.subscriptions.get(stream)!.add(callback);
    return () => {
      this.subscriptions.get(stream)?.delete(callback);
      if (this.subscriptions.get(stream)?.size === 0) {
        this.subscriptions.delete(stream);
        if (this.ws?.readyState === WebSocket.OPEN) {
          this.send({ method: 'UNSUBSCRIBE', params: [stream], id: Date.now() });
        }
      }
    };
  }

  disconnect(): void {
    this.stopPing();
    this.maxReconnects = 0;
    this.ws?.close();
    this.ws = null;
  }

  private send(data: unknown): void {
    this.ws?.send(JSON.stringify(data));
  }

  private handleMessage(event: MessageEvent): void {
    try {
      const data = JSON.parse(event.data);
      const stream = data.stream || data.e;
      if (stream && this.subscriptions.has(stream)) {
        this.subscriptions.get(stream)!.forEach(cb => cb(data));
      }
    } catch {
      // Ignore malformed messages
    }
  }

  private reconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnects) return;
    this.reconnectAttempts++;
    setTimeout(() => this.connect(this.url), this.reconnectDelay * this.reconnectAttempts);
  }

  private startPing(): void {
    this.pingInterval = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send('ping');
      }
    }, 30000);
  }

  private stopPing(): void {
    if (this.pingInterval) {
      clearInterval(this.pingInterval);
      this.pingInterval = null;
    }
  }
}

export const wsManager = new WebSocketManager();
