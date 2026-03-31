"use client";

import dynamic from "next/dynamic";

const ChartInner = dynamic(() => import("./ChartInner"), { ssr: false });

export default function Chart() {
  return <ChartInner />;
}
