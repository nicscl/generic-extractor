import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Extractor",
  description: "Document and sheet extraction with AI",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className="dark">
      <body className="bg-zinc-950 text-white antialiased">{children}</body>
    </html>
  );
}
