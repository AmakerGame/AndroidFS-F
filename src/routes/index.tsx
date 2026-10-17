import { createFileRoute } from "@tanstack/react-router";
import { HardDrive, ShieldCheck, Usb } from "lucide-react";

export const Route = createFileRoute("/")({ component: Home });

const fixes = [
  {
    title: "Демон більше не падає на записі",
    text: "Створення файлу й папки було незакінченим і обривало процес на телефоні. Читання за кінцем файлу теж валило диск.",
  },
  {
    title: "Провідник отримує правильні відповіді",
    text: "Немає файлу — це «не знайдено», а не внутрішня помилка. Видалення відбувається при закритті ручки, а не рекурсивно і не завчасно.",
  },
  {
    title: "Телефон знову з’являється як диск",
    text: "adb шукається у SDK, старий демон зупиняється перед заливкою, буква диска бронюється, а корінь — це пам’ять телефону, не системний каталог /.",
  },
];

function Home() {
  return (
    <main className="mx-auto flex min-h-screen w-full max-w-3xl flex-col gap-8 px-5 py-10 sm:px-8">
      <header className="flex flex-col gap-5">
        <p className="font-mono text-sm tracking-wide text-accent">AndroidFS-F 0.2</p>
        <h1 className="max-w-xl text-4xl font-semibold leading-tight text-fg sm:text-5xl">
          Вихідний код драйвера виправлено
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-muted">
          Це той самий AndroidFS-F: телефон монтується в Windows як диск через ADB.
          Веб-сторінка лише показує, що змінилось у коді. Сам диск Q: з’являється
          на комп’ютері після збірки.
        </p>
      </header>

      <section className="grid gap-3 sm:grid-cols-3">
        <article className="rounded-card border border-line bg-surface p-4">
          <Usb className="mb-3 size-5 text-accent" aria-hidden />
          <h2 className="text-base font-medium text-fg">USB-налагодження</h2>
          <p className="mt-1 text-sm leading-relaxed text-muted">Телефон у стані device, не unauthorized.</p>
        </article>
        <article className="rounded-card border border-line bg-surface p-4">
          <HardDrive className="mb-3 size-5 text-accent" aria-hidden />
          <h2 className="text-base font-medium text-fg">Літера Q:</h2>
          <p className="mt-1 text-sm leading-relaxed text-muted">Перша вільна, далі R:, S: і наступні.</p>
        </article>
        <article className="rounded-card border border-line bg-surface p-4">
          <ShieldCheck className="mb-3 size-5 text-accent" aria-hidden />
          <h2 className="text-base font-medium text-fg">Корінь диска</h2>
          <p className="mt-1 text-sm leading-relaxed text-muted">Внутрішня пам’ять, не весь системний розділ.</p>
        </article>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-sm font-medium tracking-wide text-muted">Що саме полагоджено</h2>
        <ol className="flex flex-col gap-3">
          {fixes.map((item, index) => (
            <li key={item.title} className="rounded-card border border-line bg-surface px-4 py-4">
              <p className="font-mono text-xs text-accent">0{index + 1}</p>
              <h3 className="mt-1 text-lg font-medium text-fg">{item.title}</h3>
              <p className="mt-1 text-sm leading-relaxed text-muted">{item.text}</p>
            </li>
          ))}
        </ol>
      </section>

      <section className="rounded-card border border-line bg-surface-2 p-5">
        <h2 className="text-lg font-medium text-fg">Як зібрати на Windows</h2>
        <ol className="mt-3 flex list-decimal flex-col gap-2 pl-5 text-sm leading-relaxed text-muted">
          <li>Dokan 1.5.1 і Android platform-tools (adb.exe у PATH).</li>
          <li>Стабільний Rust, Android NDK у ANDROID_NDK_HOME, cargo-ndk.</li>
          <li>
            У каталозі проєкту запустити <span className="font-mono text-fg">build.ps1</span>.
            Спочатку збирається демон для телефону, потім драйвер.
          </li>
          <li>
            Перевірка без телефону: <span className="font-mono text-fg">cargo test</span>.
          </li>
        </ol>
        <p className="mt-4 text-sm leading-relaxed text-muted">
          Повний список змін — у README поруч із вихідним кодом. Лог, якщо диска не
          видно: файл androidfs.log біля драйвера, або вікно консолі через
          ANDROIDFS_CONSOLE=1.
        </p>
      </section>
    </main>
  );
}
