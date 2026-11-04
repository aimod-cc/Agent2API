import { useCallback, useRef, useState } from 'react'
import { t } from '../i18n'
import { toast } from './assistant-state'
import { prepareImage } from './assistant-images'
import {
  MAX_TEXT_FILES,
  MAX_TEXT_FILE_BYTES,
  type AssistantTextFile,
} from '../../../ui-shared/assistant-files'

const MAX_IMAGES = 6
const MAX_IMAGE_BYTES = 10 * 1024 * 1024

/** JSON / .info 都按文本读；Windows 上 .json 的 MIME 经常是空的，所以看扩展名 */
function isTextCredentialFile(file: File): boolean {
  return /\.(json|info)$/i.test(file.name) || file.type === 'application/json'
}

export function useAssistantAttachments(session: { current: string }) {
  const [images, setImages] = useState<string[]>([])
  const [files, setFiles] = useState<AssistantTextFile[]>([])
  const imagesRef = useRef(images)
  imagesRef.current = images
  const filesRef = useRef(files)
  filesRef.current = files
  const pendingImagesRef = useRef(0)
  const pendingFilesRef = useRef(0)
  const [reading, setReading] = useState(false)

  const syncReading = useCallback(() => {
    setReading(pendingImagesRef.current > 0 || pendingFilesRef.current > 0)
  }, [])

  const addFiles = useCallback((incoming: FileList | File[]) => {
    const list = Array.from(incoming)
    const imageFiles = list.filter(file => file.type.startsWith('image/'))
    const textFiles = list.filter(file => !file.type.startsWith('image/') && isTextCredentialFile(file))
    const unsupported = list.filter(file => !file.type.startsWith('image/') && !isTextCredentialFile(file))
    if (unsupported.length) {
      toast(t('只支持图片和 JSON 凭证文件（.json / .info）；「{name}」已跳过。', { name: unsupported[0].name }), 'err')
    }

    /* ── 图片（原有链路）────────────────────────────────── */
    if (imageFiles.length) {
      const oversized = imageFiles.find(file => file.size > MAX_IMAGE_BYTES)
      if (oversized) {
        toast(t('「{name}」超过 {n} MB，换一张小一点的。', { name: oversized.name, n: MAX_IMAGE_BYTES / 1024 / 1024 }), 'err')
      } else {
        const room = MAX_IMAGES - imagesRef.current.length - pendingImagesRef.current
        if (room <= 0) {
          toast(t('一次最多 {n} 张图片。', { n: MAX_IMAGES }), 'err')
        } else {
          const take = imageFiles.slice(0, room)
          if (take.length < imageFiles.length) toast(t('一次最多 {n} 张图片，多出的没有添加。', { n: MAX_IMAGES }), 'warn')
          const owner = session.current
          pendingImagesRef.current += take.length
          syncReading()
          for (const file of take) {
            void prepareImage(file).then(url => {
              if (url && session.current === owner) setImages(current => current.length >= MAX_IMAGES ? current : [...current, url])
            }).catch(error => toast(t('读取图片失败：{message}', { message: error instanceof Error ? error.message : String(error) }), 'err'))
              .finally(() => {
                pendingImagesRef.current = Math.max(0, pendingImagesRef.current - 1)
                syncReading()
              })
          }
        }
      }
    }

    /* ── JSON / .info（文本附件）────────────────────────── */
    if (textFiles.length) {
      const oversized = textFiles.find(file => file.size > MAX_TEXT_FILE_BYTES)
      if (oversized) {
        toast(t('「{name}」超过 {n} MB，请换一个小一点的 JSON 文件，或去添加账号页导入。', {
          name: oversized.name,
          n: MAX_TEXT_FILE_BYTES / 1024 / 1024,
        }), 'err')
      } else {
        const room = MAX_TEXT_FILES - filesRef.current.length - pendingFilesRef.current
        if (room <= 0) {
          toast(t('一次最多 {n} 个文件。', { n: MAX_TEXT_FILES }), 'err')
        } else {
          const take = textFiles.slice(0, room)
          if (take.length < textFiles.length) toast(t('一次最多 {n} 个文件，多出的没有添加。', { n: MAX_TEXT_FILES }), 'warn')
          const owner = session.current
          pendingFilesRef.current += take.length
          syncReading()
          for (const file of take) {
            void file.text().then(raw => {
              const text = raw.replace(/^\uFEFF/, '')
              if (session.current !== owner) return
              if (!text.trim()) {
                toast(t('「{name}」是空文件。', { name: file.name }), 'err')
                return
              }
              if (text.includes('\u0000')) {
                toast(t('「{name}」看起来是二进制文件，助手只接收 JSON 文本。', { name: file.name }), 'err')
                return
              }
              setFiles(current => current.length >= MAX_TEXT_FILES ? current : [...current, {
                name: file.name,
                size: file.size,
                text,
              }])
            }).catch(error => {
              toast(t('读取文件失败：{message}', { message: error instanceof Error ? error.message : String(error) }), 'err')
            }).finally(() => {
              pendingFilesRef.current = Math.max(0, pendingFilesRef.current - 1)
              syncReading()
            })
          }
        }
      }
    }
  }, [session, syncReading])

  return { images, setImages, files, setFiles, addFiles, reading }
}
