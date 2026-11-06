// The application's single summary (stat) card: one labelled figure such as a
// balance, a credit limit or a reward-point total, on the dashboard, the
// rewards page and the bill payment page.
//
// `value` is the decimal string the API returns and is rendered exactly as
// received. It is never converted to a JavaScript number or reformatted: a
// stored balance such as 9007199254740993 lies beyond the range a double holds
// exactly and must reach the screen digit for digit.
//
// A figure is one unbroken line, since a number split across two lines reads
// as two numbers. A figure of up to LONG_FIGURE_LENGTH characters, such as
// 2342 or -1020.00, is text-2xl at every width. A longer one, such as a
// balance of -9999999999.99 or a 19-digit point total up to
// 9223372036854775807, takes its type size from the width of the card's
// content box (the @container on CardContent): text-sm below 16rem (@3xs),
// text-xl from 16rem and text-2xl from 20rem (@xs), on the 2rem line of
// text-2xl (leading-8), so the cards of a row keep their descriptions level.
// Either way it fits the narrowest card any page lays out from 768px up
// (about 174px, three cards in a row at 1024px beside a classic scrollbar).
// The length is that of the string as received; the figure is never parsed.
import { useId, type JSX, type ReactNode } from "react";

import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

// The longest figure shown at text-2xl in every card. At 24px, ten characters
// take at most 168px even at 0.7em a character, about the width of the
// widest fallback sans-serif digits (DejaVu Sans Bold), so they fit the
// narrowest content box (about 174px); eleven could take 185px.
const LONG_FIGURE_LENGTH = 10;

export type SummaryCardProps = {
  title: string;
  value: string;
  description?: string;
  icon?: ReactNode;
  className?: string;
};

export function SummaryCard({
  title,
  value,
  description,
  icon,
  className,
}: SummaryCardProps): JSX.Element {
  // CardTitle and the value are plain divs and a paragraph, so the group role
  // and its label are what tie the figure to its title for assistive
  // technology. Page headings stay with PageHeader, so the title is no heading.
  const titleId = useId();

  return (
    <Card role="group" aria-labelledby={titleId} className={cn(className)}>
      <CardHeader>
        <CardTitle
          id={titleId}
          className="text-sm font-medium text-muted-foreground"
        >
          {title}
        </CardTitle>
        {icon ? (
          <CardAction aria-hidden="true" className="text-muted-foreground">
            {icon}
          </CardAction>
        ) : null}
      </CardHeader>
      {/* The description sits under the value rather than in the header, so
          the values of a row of cards line up whether or not each card has
          one. The value never wraps; past LONG_FIGURE_LENGTH characters its
          size steps with the content box. */}
      <CardContent className="@container flex flex-col gap-1">
        <p
          className={cn(
            "font-semibold whitespace-nowrap tabular-nums",
            value.length > LONG_FIGURE_LENGTH
              ? "text-sm leading-8 @3xs:text-xl @xs:text-2xl"
              : "text-2xl",
          )}
        >
          {value}
        </p>
        {description ? (
          <CardDescription>{description}</CardDescription>
        ) : null}
      </CardContent>
    </Card>
  );
}

export type SummaryCardSkeletonProps = {
  /** Whether the card it stands in for shows a description. */
  description?: boolean;
  className?: string;
};

/**
 * A SummaryCard while its figure is read: the same card, header and content
 * with a pulse bar as high as each text line of the loaded card (h-5 for the
 * text-sm title and description, h-8 for the 2rem line every value takes),
 * so the card and whatever follows it keep their place when the figure
 * arrives. Hidden from assistive technology; the page tells of the read.
 */
export function SummaryCardSkeleton({
  description = false,
  className,
}: SummaryCardSkeletonProps): JSX.Element {
  return (
    <Card aria-hidden="true" className={cn(className)}>
      <CardHeader>
        <Skeleton className="h-5 w-24" />
      </CardHeader>
      <CardContent className="flex flex-col gap-1">
        <Skeleton className="h-8 w-32" />
        {description ? <Skeleton className="h-5 w-28" /> : null}
      </CardContent>
    </Card>
  );
}
