// The application's single summary (stat) card: one labelled figure such as a
// balance, a credit limit or a reward-point total, on the dashboard and the
// rewards page.
//
// `value` is the decimal string the API returns and is rendered exactly as
// received. It is never converted to a JavaScript number or reformatted: a
// stored balance such as 9007199254740993 lies beyond the range a double holds
// exactly and must reach the screen digit for digit.
import { useId, type JSX, type ReactNode } from "react";

import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { cn } from "@/lib/utils";

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
          one. */}
      <CardContent className="flex flex-col gap-1">
        <p className="text-2xl font-semibold tabular-nums wrap-anywhere">
          {value}
        </p>
        {description ? (
          <CardDescription>{description}</CardDescription>
        ) : null}
      </CardContent>
    </Card>
  );
}
