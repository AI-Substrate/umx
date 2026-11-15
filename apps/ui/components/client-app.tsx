"use client";
import dynamic from "next/dynamic";

// The page holds no server-rendered content: everything comes from the API after the token exchange. Rendering
// on the client only also keeps style attributes out of the HTML, so the CSP needs no 'unsafe-inline'.
export const ClientApp = dynamic(() => import("@/components/graph-app").then((m) => m.GraphApp), { ssr: false });
