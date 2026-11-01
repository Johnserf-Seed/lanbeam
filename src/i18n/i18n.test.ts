// The page's language attribute follows the app's language (L27).
import { expect, it } from "vitest";
import i18n from "./index";

it("<html lang> follows the language", async () => {
  await i18n.changeLanguage("en");
  expect(document.documentElement.lang).toBe("en");
  await i18n.changeLanguage("zh");
  expect(document.documentElement.lang).toBe("zh-CN");
});
