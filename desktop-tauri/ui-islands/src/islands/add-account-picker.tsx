import * as React from 'react'
import { DialogBody, DialogFooter } from '@ui'
import { ImportFootActions, ImportPanel } from './add-provider-import'
import { IMPORT_SEGMENT_ENABLED, PickStep, TYPE_ALL, TYPE_IMPORT, type AccountType } from './add-provider-pick'

export const AddAccountPicker = React.memo(function AddAccountPicker({ version, onPick }: {
  version: number
  onPick: (provider: string) => void
}) {
  const [accountType, setAccountType] = React.useState<AccountType>(TYPE_ALL)
  const [search, setSearch] = React.useState('')
  const importing = accountType === TYPE_IMPORT
  return <>
    <DialogBody>
      <PickStep accountType={accountType} onAccountTypeChange={setAccountType}
        search={search} onSearchChange={setSearch} onPick={onPick} version={version}
        importing={importing} importPanel={IMPORT_SEGMENT_ENABLED ? <ImportPanel segmentOn={importing} /> : undefined} />
    </DialogBody>
    {IMPORT_SEGMENT_ENABLED && importing ? <DialogFooter><ImportFootActions /></DialogFooter> : null}
  </>
})
