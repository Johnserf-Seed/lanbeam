// 信任圈 (src/pages/TrustedPage.tsx): a dashed ring around 本机; devices
// inside are trusted and auto-accepted.
import { lerp } from "../lib/anim";
import { C, FONT, MONO } from "../lib/theme";

export type TrustNode = {
  id: string;
  name: string;
  x: number;
  y: number;
  trusted: number; // 0..1 (the label + dot colour follow it)
  auto: boolean;
  mirror?: boolean;
  lift?: number; // 0..1 while dragged
  appear?: number; // 0..1+ (spring)
};

export const TRUST_R = 150;

export function TrustCircle({ nodes, hot, ring = 1 }: { nodes: TrustNode[]; hot: number; ring?: number }) {
  const R = TRUST_R * (0.6 + 0.4 * ring);
  return (
    <div style={{ position: "relative", width: 0, height: 0, fontFamily: FONT }}>
      <div
        style={{
          position: "absolute",
          width: R * 2,
          height: R * 2,
          margin: `${-R}px 0 0 ${-R}px`,
          borderRadius: "50%",
          background: C.accentSoft,
          opacity: (0.5 + 0.4 * hot) * ring,
          boxShadow: hot ? `0 0 ${50 * hot}px rgba(127,212,212,${0.18 * hot})` : undefined,
        }}
      />
      <div
        style={{
          position: "absolute",
          width: R * 2,
          height: R * 2,
          margin: `${-R}px 0 0 ${-R}px`,
          borderRadius: "50%",
          border: `1px dashed ${C.accent}`,
          opacity: (0.5 + 0.5 * hot) * ring,
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 0,
          top: -R,
          transform: "translate(-50%, -50%)",
          padding: "0 6px",
          whiteSpace: "nowrap",
          lineHeight: "14px",
          fontFamily: MONO,
          fontSize: 10.5,
          color: C.accentInk,
          background: C.void,
          opacity: ring,
          zIndex: 1,
        }}
      >
        信任圈
      </div>
      <div
        style={{
          position: "absolute",
          margin: "-23px 0 0 -23px",
          width: 46,
          height: 46,
          borderRadius: "50%",
          background: C.panel,
          border: `1px solid ${C.border2}`,
          boxShadow: `0 0 18px ${C.ringA}`,
          display: "grid",
          placeItems: "center",
          fontSize: 11,
          fontWeight: 650,
          color: C.accentInk,
          opacity: ring,
          zIndex: 1,
        }}
      >
        本机
      </div>
      {nodes.map((n) => {
        const tr = n.trusted;
        const lift = n.lift ?? 0;
        const ap = n.appear ?? 1;
        if (ap <= 0.001) return null;
        const dotColor = tr > 0.5 ? C.accent : C.muted;
        return (
          <div
            key={n.id}
            style={{
              position: "absolute",
              left: n.x,
              top: n.y,
              width: 0,
              height: 0,
              zIndex: 2 + (lift > 0 ? 5 : 0),
              opacity: Math.min(1, ap * 1.3),
            }}
          >
            <div
              style={{
                position: "absolute",
                width: 12,
                height: 12,
                margin: "-6px 0 0 -6px",
                borderRadius: "50%",
                background: dotColor,
                boxShadow: tr > 0.5 ? `0 0 12px ${C.glow}` : undefined,
                transform: `scale(${ap * (1 + lift * 0.35)})`,
              }}
            />
            <div
              style={{
                position: "absolute",
                top: -17,
                ...(n.mirror ? { right: 12 } : { left: 12 }),
                textAlign: n.mirror ? "right" : "left",
                whiteSpace: "nowrap",
                translate: `${(1 - Math.min(1, ap)) * (n.mirror ? 8 : -8)}px 0`,
              }}
            >
              <div style={{ fontSize: 12.5, fontWeight: 600, color: C.ink2, lineHeight: "18px" }}>{n.name}</div>
              <div style={{ fontSize: 10.5, lineHeight: "14px", position: "relative", height: 14 }}>
                <span style={{ position: "absolute", right: n.mirror ? 0 : undefined, color: C.muted, opacity: 1 - tr, transform: `translateY(${-tr * 6}px)` }}>
                  每次询问
                </span>
                <span
                  style={{
                    position: "absolute",
                    right: n.mirror ? 0 : undefined,
                    opacity: tr,
                    transform: `translateY(${lerp(6, 0, tr)}px)`,
                  }}
                >
                  <span style={{ color: C.accentInk, fontWeight: 600 }}>已信任</span>
                  <span style={{ color: C.muted }}> · </span>
                  <span style={{ color: n.auto ? C.accentInk : C.muted, fontWeight: n.auto ? 600 : 400 }}>
                    {n.auto ? "自动接收" : "每次询问"}
                  </span>
                </span>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
