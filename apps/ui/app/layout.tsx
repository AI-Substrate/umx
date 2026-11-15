import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import type { Metadata } from "next";
import { headers } from "next/headers";
import { ThemeProvider } from "@/components/theme";
import "./globals.css";

export const metadata: Metadata = { title: "umx graph view", referrer: "no-referrer" };

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  return (
    <html lang="en" suppressHydrationWarning className={`${GeistSans.variable} ${GeistMono.variable}`}>
      <body className="font-sans">
        <ThemeProvider nonce={nonce}>{children}</ThemeProvider>
      </body>
    </html>
  );
}
