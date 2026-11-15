import type { NextConfig } from "next";

const root = import.meta.dirname;

const config: NextConfig = {
  output: "standalone",
  outputFileTracingRoot: root,
  turbopack: { root },
  poweredByHeader: false,
  productionBrowserSourceMaps: false,
  reactStrictMode: true,
  // No image optimiser, no remote anything.
  images: { unoptimized: true },
};

export default config;
