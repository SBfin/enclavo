export default function PageHeader({ title, note }: { title: string; note?: string }) {
  return (
    <div className="mb-8 flex flex-wrap items-baseline justify-between gap-2">
      <h1 className="text-3xl font-light tracking-tight">{title}</h1>
      {note && <span className="label">{note}</span>}
    </div>
  );
}
