import Header from "@/components/layout/Header";

export default function TradingLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col h-screen">
      <Header />
      <main className="flex-1 min-h-0">{children}</main>
    </div>
  );
}
