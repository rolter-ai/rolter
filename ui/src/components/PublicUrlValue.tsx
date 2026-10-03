import { Trans, useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { LoadingRegion } from "@/components/LoadingState";
import { COPYABLE_BOX_HEIGHT, CopyableValue } from "@/components/ui/copyable-value";
import { Skeleton } from "@/components/ui/skeleton";
import { usePublicUrl } from "@/lib/use-public-url";

/**
 * An address built on the control plane's public base, as a copyable value
 * (#2366).
 *
 * The control plane builds every address it gives an outside caller from
 * `ROLTER_PUBLIC_URL`, never from the request, and the dashboard may be open
 * under a different name than the one the caller must use. So the base is read
 * from the control plane through `usePublicUrl()` and `address` builds the
 * value from it, never from `window.location`.
 *
 * The read does not gate the surface around it, and its three states are said
 * here once rather than by each screen: pending holds the box's space as a
 * skeleton, a failed read says so with a retry instead of a URL that might be
 * wrong, and an unset `ROLTER_PUBLIC_URL` still shows the default address,
 * copyable, with a note under it that only a caller on the control plane's own
 * host can reach it. `CopyableValue` stays free of the query; this is the one
 * place the two meet.
 */
export function PublicUrlValue({
  address,
  label,
  copyLabel,
  hint,
  testId,
  className,
}: {
  /** the value, from the control plane's public base */
  address: (base: string) => string;
  label: string;
  copyLabel: string;
  hint?: string;
  testId?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const publicUrl = usePublicUrl();
  const value = publicUrl.data ? address(publicUrl.data.public_url) : null;
  return (
    <CopyableValue
      label={label}
      value={value}
      copyLabel={copyLabel}
      hint={hint}
      testId={testId}
      className={className}
      status={
        publicUrl.isError ? (
          <LoadError
            error={publicUrl.error}
            resource={t("errors.resources.publicUrl")}
            onRetry={() => void publicUrl.refetch()}
            target="public-url"
          />
        ) : value ? undefined : (
          <LoadingRegion className="w-full">
            <Skeleton height={COPYABLE_BOX_HEIGHT} radius={6} />
          </LoadingRegion>
        )
      }
      note={
        publicUrl.data?.configured === false && (
          <Trans
            i18nKey="common.publicUrl.unset"
            values={{ url: publicUrl.data.public_url }}
            components={{ code: <code className="font-mono" /> }}
          />
        )
      }
    />
  );
}
