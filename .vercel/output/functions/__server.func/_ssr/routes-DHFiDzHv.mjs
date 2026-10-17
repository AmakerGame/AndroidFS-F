import { b as require_jsx_runtime } from "../_libs/@tanstack/react-router+[...].mjs";
import { i as HardDrive, r as ShieldCheck, t as Usb } from "../_libs/lucide-react.mjs";
//#region node_modules/.nitro/vite/services/ssr/assets/routes-DHFiDzHv.js
var import_jsx_runtime = require_jsx_runtime();
var fixes = [
	{
		title: "Демон більше не падає на записі",
		text: "Створення файлу й папки було незакінченим і обривало процес на телефоні. Читання за кінцем файлу теж валило диск."
	},
	{
		title: "Провідник отримує правильні відповіді",
		text: "Немає файлу — це «не знайдено», а не внутрішня помилка. Видалення відбувається при закритті ручки, а не рекурсивно і не завчасно."
	},
	{
		title: "Телефон знову з’являється як диск",
		text: "adb шукається у SDK, старий демон зупиняється перед заливкою, буква диска бронюється, а корінь — це пам’ять телефону, не системний каталог /."
	}
];
function Home() {
	return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("main", {
		className: "mx-auto flex min-h-screen w-full max-w-3xl flex-col gap-8 px-5 py-10 sm:px-8",
		children: [
			/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("header", {
				className: "flex flex-col gap-5",
				children: [
					/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
						className: "font-mono text-sm tracking-wide text-accent",
						children: "AndroidFS-F 0.2"
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h1", {
						className: "max-w-xl text-4xl font-semibold leading-tight text-fg sm:text-5xl",
						children: "Вихідний код драйвера виправлено"
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
						className: "max-w-2xl text-lg leading-relaxed text-muted",
						children: "Це той самий AndroidFS-F: телефон монтується в Windows як диск через ADB. Веб-сторінка лише показує, що змінилось у коді. Сам диск Q: з’являється на комп’ютері після збірки."
					})
				]
			}),
			/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("section", {
				className: "grid gap-3 sm:grid-cols-3",
				children: [
					/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("article", {
						className: "rounded-card border border-line bg-surface p-4",
						children: [
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)(Usb, {
								className: "mb-3 size-5 text-accent",
								"aria-hidden": true
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h2", {
								className: "text-base font-medium text-fg",
								children: "USB-налагодження"
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
								className: "mt-1 text-sm leading-relaxed text-muted",
								children: "Телефон у стані device, не unauthorized."
							})
						]
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("article", {
						className: "rounded-card border border-line bg-surface p-4",
						children: [
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)(HardDrive, {
								className: "mb-3 size-5 text-accent",
								"aria-hidden": true
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h2", {
								className: "text-base font-medium text-fg",
								children: "Літера Q:"
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
								className: "mt-1 text-sm leading-relaxed text-muted",
								children: "Перша вільна, далі R:, S: і наступні."
							})
						]
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("article", {
						className: "rounded-card border border-line bg-surface p-4",
						children: [
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)(ShieldCheck, {
								className: "mb-3 size-5 text-accent",
								"aria-hidden": true
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h2", {
								className: "text-base font-medium text-fg",
								children: "Корінь диска"
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
								className: "mt-1 text-sm leading-relaxed text-muted",
								children: "Внутрішня пам’ять, не весь системний розділ."
							})
						]
					})
				]
			}),
			/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("section", {
				className: "flex flex-col gap-3",
				children: [/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h2", {
					className: "text-sm font-medium tracking-wide text-muted",
					children: "Що саме полагоджено"
				}), /* @__PURE__ */ (0, import_jsx_runtime.jsx)("ol", {
					className: "flex flex-col gap-3",
					children: fixes.map((item, index) => /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("li", {
						className: "rounded-card border border-line bg-surface px-4 py-4",
						children: [
							/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("p", {
								className: "font-mono text-xs text-accent",
								children: ["0", index + 1]
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h3", {
								className: "mt-1 text-lg font-medium text-fg",
								children: item.title
							}),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
								className: "mt-1 text-sm leading-relaxed text-muted",
								children: item.text
							})
						]
					}, item.title))
				})]
			}),
			/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("section", {
				className: "rounded-card border border-line bg-surface-2 p-5",
				children: [
					/* @__PURE__ */ (0, import_jsx_runtime.jsx)("h2", {
						className: "text-lg font-medium text-fg",
						children: "Як зібрати на Windows"
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("ol", {
						className: "mt-3 flex list-decimal flex-col gap-2 pl-5 text-sm leading-relaxed text-muted",
						children: [
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("li", { children: "Dokan 1.5.1 і Android platform-tools (adb.exe у PATH)." }),
							/* @__PURE__ */ (0, import_jsx_runtime.jsx)("li", { children: "Стабільний Rust, Android NDK у ANDROID_NDK_HOME, cargo-ndk." }),
							/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("li", { children: [
								"У каталозі проєкту запустити ",
								/* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", {
									className: "font-mono text-fg",
									children: "build.ps1"
								}),
								". Спочатку збирається демон для телефону, потім драйвер."
							] }),
							/* @__PURE__ */ (0, import_jsx_runtime.jsxs)("li", { children: [
								"Перевірка без телефону: ",
								/* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", {
									className: "font-mono text-fg",
									children: "cargo test"
								}),
								"."
							] })
						]
					}),
					/* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", {
						className: "mt-4 text-sm leading-relaxed text-muted",
						children: "Повний список змін — у README поруч із вихідним кодом. Лог, якщо диска не видно: файл androidfs.log біля драйвера, або вікно консолі через ANDROIDFS_CONSOLE=1."
					})
				]
			})
		]
	});
}
//#endregion
export { Home as component };
