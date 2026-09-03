import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"

interface ConfirmationDialogProps {
  open: boolean
  title: string
  description: string
  confirmLabel: string
  pendingLabel: string
  destructive?: boolean
  isPending?: boolean
  error?: string | null
  onConfirm: () => void | Promise<void>
  onOpenChange: (open: boolean) => void
}

export function ConfirmationDialog({
  open,
  title,
  description,
  confirmLabel,
  pendingLabel,
  destructive = false,
  isPending = false,
  error,
  onConfirm,
  onOpenChange,
}: ConfirmationDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={(nextOpen) => {
      if (!isPending) onOpenChange(nextOpen)
    }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <AlertDialogFooter>
          <AlertDialogCancel type="button" disabled={isPending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            type="button"
            variant={destructive ? "destructive" : "default"}
            className={destructive ? undefined : "bg-violet-600 text-white hover:bg-violet-500"}
            disabled={isPending}
            onClick={() => void onConfirm()}
          >
            {isPending ? pendingLabel : confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
