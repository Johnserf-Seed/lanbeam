// 配对设备 (src/components/PairModal.tsx): the host view (address + 6-digit
// code), the join view, and the code comparison both screens show.
import type { CSSProperties, ReactNode } from "react";
import { lerp } from "../lib/anim";
import { C, MONO } from "../lib/theme";
import { Btn, Modal } from "./primitives";

const fieldLabel: CSSProperties = {
  fontSize: 10.5,
  fontWeight: 600,
  letterSpacing: ".07em",
  color: C.muted,
};

/** `role`: 0 = 让对方加入我, 1 = 加入对方 (fractional while the pill slides). */
function Segmented({ role }: { role: number }) {
  return (
    <div
      style={{
        position: "relative",
        display: "flex",
        gap: 2,
        background: C.sidebar,
        border: `1px solid ${C.border}`,
        borderRadius: 8,
        padding: 3,
      }}
    >
      <div
        style={{
          position: "absolute",
          top: 3,
          bottom: 3,
          left: `calc(3px + ${role} * (50% - 4px))`,
          width: "calc(50% - 4px)",
          borderRadius: 6,
          background: C.panel,
        }}
      />
      {["让对方加入我", "加入对方"].map((l, i) => (
        <div
          key={l}
          style={{
            position: "relative",
            flex: 1,
            textAlign: "center",
            padding: "6px 12px",
            fontSize: 12,
            fontWeight: 600,
            color: Math.abs(role - i) < 0.5 ? C.ink2 : C.muted,
          }}
        >
          {l}
        </div>
      ))}
    </div>
  );
}

export function PairPanel({
  role,
  view,
  addr,
  code,
  remaining,
  joinAddr,
  joinCode,
  linkChip,
  joinPress = 0,
  compareSas,
  matchPress = 0,
}: {
  role: number;
  view: "host" | "join" | "compare";
  addr: ReactNode;
  code: ReactNode;
  remaining: string;
  joinAddr: string;
  joinCode: string;
  linkChip: number;
  joinPress?: number;
  compareSas?: ReactNode;
  matchPress?: number;
}) {
  if (view === "compare")
    return (
      <Modal width={500} title="已连接到「书房 · MacBook Pro」" sub="你和「书房 · MacBook Pro」的屏幕现在应显示同一组数字" foot="校验码一致后双方互存指纹 · 以后同网自动识别并自动接收">
        <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 8, padding: "18px 20px 18px" }}>
          <div style={fieldLabel}>核对校验码</div>
          <div
            style={{
              fontFamily: MONO,
              fontSize: 26,
              fontWeight: 600,
              letterSpacing: ".06em",
              color: C.accentInk,
              background: C.accentSoft,
              borderRadius: 10,
              padding: "10px 18px",
              margin: "2px 0",
              display: "flex",
            }}
          >
            {compareSas}
          </div>
          <div style={{ fontSize: 10.5, color: C.muted, textAlign: "center", lineHeight: 1.6 }}>
            只有一致，才说明没有第三方在中间转发
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 6, width: "100%" }}>
            <Btn kind="danger" style={{ flex: "none", padding: "0 14px" }}>
              不一致
            </Btn>
            <Btn kind="primary" press={matchPress} style={{ flex: 1 }}>
              一致 · 信任这台设备
            </Btn>
          </div>
        </div>
      </Modal>
    );

  return (
    <Modal width={500} title="配对设备" sub="核对校验码，让两台电脑互相信任 · 自动发现看不到对方时也能用" foot="校验码一致后双方互存指纹 · 以后同网自动识别并自动接收">
      <div style={{ padding: "14px 20px 0" }}>
        <Segmented role={role} />
      </div>
      {view === "host" ? (
        <div style={{ padding: "14px 20px 16px" }}>
          <div style={{ fontSize: 11.5, color: C.muted2, lineHeight: 1.6 }}>
            在另一台电脑的 LanBeam 里点「配对 → 加入对方」，填入这两项：
          </div>
          <div style={{ display: "flex", gap: 12, marginTop: 12 }}>
            <div style={{ flex: 1.35, background: C.sidebar, borderRadius: 10, padding: "10px 12px 12px" }}>
              <div style={fieldLabel}>本机地址</div>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginTop: 8 }}>
                <span style={{ fontFamily: MONO, fontSize: 14.5, fontWeight: 600, color: C.ink2 }}>{addr}</span>
                <span style={{ fontSize: 11, fontWeight: 600, color: C.accentInk }}>复制</span>
              </div>
            </div>
            <div style={{ flex: 1, background: C.sidebar, borderRadius: 10, padding: "10px 12px 12px" }}>
              <div style={fieldLabel}>配对码</div>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginTop: 4 }}>
                <span style={{ fontFamily: MONO, fontSize: 21, fontWeight: 600, color: C.accentInk, letterSpacing: ".06em", display: "flex" }}>
                  {code}
                </span>
                <span style={{ fontSize: 11, fontWeight: 600, color: C.accentInk }}>复制</span>
              </div>
            </div>
          </div>
          <div style={{ fontSize: 11, color: C.muted, marginTop: 10 }}>
            {remaining} · <span style={{ color: C.accentInk }}>换一个</span>
          </div>
          <div style={{ borderTop: `1px solid ${C.border}`, marginTop: 14, paddingTop: 12, fontSize: 11.5, color: C.muted2 }}>
            或者把配对链接发给对方：地址和配对码都在里面，对方点开就会自动填好
          </div>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginTop: 8 }}>
            <span style={{ fontFamily: MONO, fontSize: 10, color: C.muted }}>…a=192.168.1.20&amp;p=51704&amp;c=482913</span>
            <Btn style={{ height: 28, fontSize: 11.5 }}>复制配对链接</Btn>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 12, fontSize: 11, color: C.muted2 }}>
            <span style={{ width: 7, height: 7, borderRadius: "50%", background: C.accent, opacity: 0.8 }} />
            等待对方加入…
          </div>
        </div>
      ) : (
        <div style={{ padding: "14px 20px 16px" }}>
          <div style={{ fontSize: 11.5, color: C.muted2, lineHeight: 1.6 }}>
            在对方的 LanBeam 里点「配对」，把它显示的地址和配对码填到这里：
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 10, alignItems: "flex-end" }}>
            <div style={{ flex: 1 }}>
              <div style={{ ...fieldLabel, marginBottom: 5 }}>对方地址</div>
              <Field value={joinAddr} placeholder="192.168.1.__:51704" />
            </div>
            <div style={{ width: 104 }}>
              <div style={{ ...fieldLabel, marginBottom: 5 }}>配对码</div>
              <Field value={joinCode} placeholder="6 位数字" />
            </div>
            <Btn kind="primary" press={joinPress} style={{ height: 30, padding: "0 16px" }}>
              加入
            </Btn>
          </div>
          <div style={{ height: 32, marginTop: 9 }}>
            <span
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 12,
                fontSize: 11,
                fontWeight: 600,
                color: C.accentInk,
                background: C.accentSoft,
                borderRadius: 99,
                padding: "4px 11px",
                opacity: linkChip,
                transform: `translateY(${lerp(6, 0, linkChip)}px)`,
              }}
            >
              来自「书房 · MacBook Pro」的配对链接
              <span style={{ color: C.muted2, fontWeight: 500 }}>清除</span>
            </span>
          </div>
          <div style={{ borderTop: `1px solid ${C.border}`, marginTop: 6, paddingTop: 11, fontSize: 11.5, whiteSpace: "nowrap" }}>
            <span style={{ fontWeight: 600, color: C.accentInk }}>只添加设备，不配对</span>
            <span style={{ color: C.muted, marginLeft: 8, fontSize: 10.5 }}>不核对校验码、不建立信任，只是让它出现在设备列表里</span>
          </div>
        </div>
      )}
    </Modal>
  );
}

function Field({ value, placeholder }: { value: string; placeholder: string }) {
  return (
    <div
      style={{
        height: 30,
        borderRadius: 8,
        border: `1px solid ${value ? C.border2 : C.border2}`,
        background: C.panel,
        display: "flex",
        alignItems: "center",
        padding: "0 10px",
        fontFamily: MONO,
        fontSize: 12,
        color: value ? C.ink2 : C.muted,
        whiteSpace: "nowrap",
        overflow: "hidden",
      }}
    >
      {value || placeholder}
    </div>
  );
}

