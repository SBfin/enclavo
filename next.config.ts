import type { NextConfig } from "next";
import path from "node:path";

const nextConfig: NextConfig = {
  /* config options here */
  cacheComponents: true,
  partialPrefetching: true,
  turbopack: {
    rules: {
      "*.css": {
        loaders: [
          {
            loader: "@tailwindcss/turbopack",
            // Class names live in src; don't scan Rust sources and build artifacts.
            options: { base: path.join(process.cwd(), "src") },
          },
        ],
        as: "*.css",
      },
    },
  },
};

export default nextConfig;
