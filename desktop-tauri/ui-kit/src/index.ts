/**
 * @agent2api/ui —— 项目私有组件库。
 *
 * 源码随仓库维护（不走 npm 发布），消费方是 desktop-tauri/ui-islands；
 * 样式入口见 styles/globals.css，消费方必须配 @source 扫描本目录。
 *
 * 组件的**形态、props 命名、组合方式与行为**一律对齐 shadcn/ui 的标准
 * （Base UI 那一支）：别人按 shadcn 的习惯写就能命中，不必先读我们的源码。
 * **样式取值**则一律走项目令牌（styles/theme.css），观感与既有界面一致。
 *
 * 加新组件时按这个口径来：先看 shadcn 有没有标准件，有就照它的 API 搬，
 * 不要自己发明 prop（Input 上曾有个自造的 `icon`，已由标准的 InputGroup 取代）。
 */

export { Button, buttonVariants, type ButtonProps } from './button'
export { Input } from './input'
export { Textarea } from './textarea'
export {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
  InputGroupTextarea,
} from './input-group'
export { Badge, BadgeDot, badgeVariants, type BadgeProps } from './badge'
export { Spinner } from './spinner'
export { Skeleton } from './skeleton'
export { Label } from './label'
export { Switch, type SwitchProps } from './switch'
export { Checkbox } from './checkbox'
export { Progress, ProgressTrack, ProgressIndicator, ProgressLabel, ProgressValue } from './progress'
export { Tooltip, TooltipProvider, TooltipTrigger, TooltipContent, TooltipArrow, type TooltipContentProps } from './tooltip'
export {
  Dialog,
  DialogTrigger,
  DialogPortal,
  DialogClose,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogHeader,
  DialogBody,
  DialogFooter,
  DialogSection,
} from './dialog'
export {
  AlertDialog,
  AlertDialogTrigger,
  AlertDialogPortal,
  AlertDialogOverlay,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogBody,
  AlertDialogFooter,
  AlertDialogBanner,
  AlertDialogTitle,
  AlertDialogDescription,
} from './alert-dialog'
export {
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
} from './table'
export {
  RadioGroup,
  RadioGroupItem,
  type RadioGroupProps,
  type RadioGroupItemProps,
} from './radio-group'
export {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  SelectGroup,
  SelectLabel,
  SelectSeparator,
} from './select'
export {
  SegmentedControl,
  type SegmentedControlOption,
  type SegmentedControlProps,
} from './segmented-control'
export { Toggle, toggleVariants, type ToggleProps } from './toggle'
export {
  MultiSelect,
  type MultiSelectOption,
  type MultiSelectProps,
} from './multi-select'
export {
  Popover,
  PopoverTrigger,

  PopoverClose,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverDescription,
} from './popover'
export { Pager, type PagerProps } from './pager'
export { NavItem, navItemVariants, type NavItemProps } from './nav-item'
export {
  Sheet,
  SheetTrigger,
  SheetClose,
  SheetPortal,
  SheetOverlay,
  SheetContent,
  SheetTitle,
  SheetDescription,
  SheetHeader,
  SheetBody,
  SheetFooter,
} from './sheet'
export { cn } from './lib/cn'
