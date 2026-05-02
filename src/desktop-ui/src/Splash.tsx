import { useI18n } from "@va/i18n";

const LOGO = ` ██╗   ██╗ ██╗ ██████╗  ███████╗  █████╗  ██████╗   ██████╗  ██╗   ██╗ ███╗   ██╗ ██████╗
 ██║   ██║ ██║ ██╔══██╗ ██╔════╝ ██╔══██╗ ██╔══██╗ ██╔═══██╗ ██║   ██║ ████╗  ██║ ██╔══██╗
 ██║   ██║ ██║ ██████╔╝ █████╗   ███████║ ██████╔╝ ██║   ██║ ██║   ██║ ██╔██╗ ██║ ██║  ██║
 ╚██╗ ██╔╝ ██║ ██╔══██╗ ██╔══╝   ██╔══██║ ██╔══██╗ ██║   ██║ ██║   ██║ ██║╚██╗██║ ██║  ██║
  ╚████╔╝  ██║ ██████╔╝ ███████╗ ██║  ██║ ██║  ██║ ╚██████╔╝ ╚██████╔╝ ██║ ╚████║ ██████╔╝
   ╚═══╝   ╚═╝ ╚═════╝  ╚══════╝ ╚═╝  ╚═╝ ╚═╝  ╚═╝  ╚═════╝   ╚═════╝  ╚═╝  ╚═══╝ ╚═════╝`;

export function Splash({ visible }: { visible: boolean }) {
  const { t } = useI18n();

  if (!visible) return null;

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 9999,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        background: "#ffffff",
        fontFamily:
          "'SF Mono','Cascadia Code','Fira Code','JetBrains Mono',monospace",
      }}
    >
      <pre
        style={{
          fontSize: "10px",
          lineHeight: 1.1,
          color: "oklch(0.55 0.18 180)",
          userSelect: "none",
        }}
      >
        {LOGO}
      </pre>
      <div
        style={{
          marginTop: 24,
          fontSize: 12,
          color: "#999",
          letterSpacing: 4,
          textTransform: "uppercase" as const,
        }}
      >
        {t("unified runtime for ai coding agents")}
      </div>
      <div
        style={{
          marginTop: 32,
          width: 20,
          height: 20,
          border: "2px solid #e0e0e0",
          borderTopColor: "oklch(0.55 0.18 180)",
          borderRadius: "50%",
          animation: "spin 0.8s linear infinite",
        }}
      />
    </div>
  );
}
