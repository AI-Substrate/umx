"use client";
import { Check, Gauge, Monitor, Moon, SquareTerminal, Sun } from "lucide-react";
import { ThemeProvider as NextThemes, useTheme } from "next-themes";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { THEMES } from "@/lib/graph/colors";
import { cn } from "@/lib/utils";

/** The choices in the theme menu, in order. System follows the OS between Light and Dark. */
export const THEME_CHOICES = [
  ["light", "Light"],
  ["dark", "Dark"],
  ["console", "Console"],
  ["instrument", "Instrument"],
  ["system", "System"],
] as const;
export type ThemeChoice = (typeof THEME_CHOICES)[number][0];

/** The choice a stored value stands for: one of the fixed names, else System (security K1-c: a name only). */
export function themeChoice(value: unknown): ThemeChoice {
  return THEME_CHOICES.some(([name]) => name === value) ? (value as ThemeChoice) : "system";
}

export function ThemeProvider({ children, nonce }: { children: React.ReactNode; nonce?: string }) {
  return (
    <NextThemes attribute="class" themes={[...THEMES]} defaultTheme="system" enableSystem disableTransitionOnChange nonce={nonce}>
      {children}
    </NextThemes>
  );
}

const ICON = { light: Sun, dark: Moon, console: SquareTerminal, instrument: Gauge, system: Monitor } as const;
const subscribeNever = () => () => {};

export function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const mounted = useSyncExternalStore(subscribeNever, () => true, () => false);
  const current = mounted ? themeChoice(theme) : "system";
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", away);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointerdown", away);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  const Icon = ICON[current];
  return (
    <div ref={box} className="relative">
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={`Theme: ${current}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={`Theme: ${current}`}
        data-testid="theme"
        onClick={() => setOpen((o) => !o)}
      >
        <Icon />
      </Button>
      {open ? (
        <div role="menu" aria-label="Theme" className="fui-frame absolute top-full right-0 z-50 mt-1 w-40 rounded-md border bg-popover p-1 shadow-md" data-testid="theme-menu">
          {THEME_CHOICES.map(([name, label]) => {
            const ItemIcon = ICON[name];
            return (
              <button
                key={name}
                type="button"
                role="menuitemradio"
                aria-checked={current === name}
                data-value={name}
                className={cn("flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent", current === name && "font-medium")}
                onClick={() => {
                  setTheme(name);
                  setOpen(false);
                }}
              >
                <ItemIcon className="size-4 text-muted-foreground" aria-hidden />
                <span className="flex-1">{label}</span>
                {current === name ? <Check className="size-3.5" aria-hidden /> : null}
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
