import * as React from "react"

import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"

function PageShell({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      className={cn("p-3 space-y-3", className)}
      {...props}
    />
  )
}

function PageHeader({
  icon,
  title,
  description,
  actions,
}: {
  icon: React.ReactNode
  title: string
  description?: React.ReactNode
  actions?: React.ReactNode
}) {
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0">
        <h2 className="text-[13px] font-semibold flex items-center gap-1.5">
          {icon}
          {title}
        </h2>
        {description && (
          <p className="text-[11px] text-muted-foreground mt-0.5 max-w-2xl">
            {description}
          </p>
        )}
      </div>
      {actions && (
        <div className="flex items-center gap-2 shrink-0">{actions}</div>
      )}
    </div>
  )
}

function StatusBanner({
  variant = "error",
  className,
  ...props
}: React.ComponentProps<"div"> & {
  variant?: "error" | "success" | "warning"
}) {
  const variantClass =
    variant === "success"
      ? "bg-emerald-500/10 text-emerald-600"
      : variant === "warning"
        ? "bg-amber-500/10 text-amber-700"
        : "bg-destructive/10 text-destructive"

  return (
    <div
      className={cn("text-xs rounded-md px-2.5 py-1.5", variantClass, className)}
      {...props}
    />
  )
}

function SectionCard({
  icon,
  title,
  badge,
  children,
  className,
}: {
  icon?: React.ReactNode
  title?: string
  badge?: string | number
  children: React.ReactNode
  className?: string
}) {
  return (
    <section className={cn("border border-border rounded-lg overflow-hidden", className)}>
      {(title || icon || badge !== undefined) && (
        <div className="flex items-center gap-1.5 px-2.5 py-1.5 bg-muted/40 border-b border-border">
          {icon}
          {title && <span className="text-xs font-semibold">{title}</span>}
          {badge !== undefined && (
            <Badge variant="muted" className="ml-auto tabular-nums">
              {badge}
            </Badge>
          )}
        </div>
      )}
      <div className="divide-y divide-border/50">{children}</div>
    </section>
  )
}

function EmptyBlock({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      className={cn("px-3 py-4 text-center text-xs text-muted-foreground", className)}
      {...props}
    />
  )
}

export { EmptyBlock, PageHeader, PageShell, SectionCard, StatusBanner }
