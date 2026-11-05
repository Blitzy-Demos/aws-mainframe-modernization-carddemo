// The application's single read-only detail list: the captions and values a
// detail view's Card holds as a `dl` grid, one column below `lg` and two from
// `lg`, and the badge every COBOL Y/N flag is shown in.
//
// Values are rendered as received. Each column is a `minmax(0, 1fr)` track,
// each item may shrink below its content and each value wraps anywhere, so a
// value without spaces, such as a 50-character address line, wraps inside its
// column and never widens the list past its card.
import type { JSX, ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { FieldError } from "@/components/ui/field";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

export type DetailListProps = {
  /** The list's DetailItem entries, in reading order. */
  children: ReactNode;
  /** Grid placement classes for a list whose columns carry meaning. */
  className?: string;
};

/** Captions and values as a definition list. */
export function DetailList({
  children,
  className,
}: DetailListProps): JSX.Element {
  return (
    <dl
      className={cn(
        "grid grid-cols-1 gap-x-6 gap-y-4 lg:grid-cols-2",
        className,
      )}
    >
      {children}
    </dl>
  );
}

export type DetailItemProps = {
  label: string;
  /** The value; nothing for a blank one. */
  children?: ReactNode;
  /** Edit texts shown under the value, such as a route key's. */
  errors?: { message: string }[];
};

/**
 * One caption and its value. The value is at least one text line high
 * (`min-h-6`, the line height of its `text-base`), so a blank value, a badge
 * and a loading placeholder take the height of the text they stand beside or
 * in for.
 */
export function DetailItem({
  label,
  children,
  errors,
}: DetailItemProps): JSX.Element {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <dt className="text-sm text-muted-foreground">{label}</dt>
      <dd className="flex min-h-6 flex-col gap-1 font-medium wrap-anywhere tabular-nums">
        {children}
        {errors !== undefined && errors.length > 0 ? (
          <FieldError errors={errors} />
        ) : null}
      </dd>
    </div>
  );
}

/**
 * A placeholder for one line of a value while its record is read: a pulse
 * bar in a box one text line high (`h-6`), so the item keeps the height the
 * line will take. Hidden from assistive technology; the page tells of the
 * read.
 */
export function DetailValueSkeleton({
  className,
}: {
  className?: string;
}): JSX.Element {
  return (
    <div aria-hidden="true" className="flex h-6 items-center">
      <Skeleton className={cn("h-5 w-32", className)} />
    </div>
  );
}

/**
 * A COBOL Y/N flag as a badge carrying its own letter, so the state is read
 * from the text and not from colour alone: `N`, the inactive state, is the
 * destructive badge and any other letter the secondary one. A blank flag
 * renders nothing rather than an empty pill.
 */
export function FlagBadge({
  value,
  className,
}: {
  value: string;
  className?: string;
}): JSX.Element | null {
  if (value.trim() === "") {
    return null;
  }
  return (
    <Badge
      variant={value === "N" ? "destructive" : "secondary"}
      className={className}
    >
      {value}
    </Badge>
  );
}
