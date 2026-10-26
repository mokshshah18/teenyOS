# teenyOS BIOS boot loader.
#
# Sector 0 (the MBR) holds stage 1, which loads stage 2 from sectors 1-15.
# Stage 2 picks a VESA graphics mode, loads the kernel file from the FAT
# partition (its position is patched into `params` when the disk is built),
# and starts it with the Linux 16-bit boot protocol.
#
# Memory: 0x7c00 loader, 0x0600 VBE info, 0x0800 VBE mode info,
#         0x10000 kernel setup code + heap, 0x1e000 command line,
#         0x20000 disk bounce buffer, 0x100000 protected-mode kernel.

	.code16
	.intel_syntax noprefix
	.text
	.globl _start

_start:
	cli
	xor ax, ax
	mov ds, ax
	mov es, ax
	mov ss, ax
	mov sp, 0x7c00
	sti
	.byte 0xea			# far jump to 0:stage1, normalising CS
	.word stage1, 0
stage1:
	mov [boot_drive], dl
	mov ah, 0x41			# LBA extensions present?
	mov bx, 0x55aa
	int 0x13
	jc no_lba
	cmp bx, 0xaa55
	jne no_lba
	mov si, offset dap1
	mov ah, 0x42
	mov dl, [boot_drive]
	int 0x13
	jc disk_error
	jmp stage2

no_lba:
	mov si, offset msg_lba
	jmp die
disk_error:
	mov si, offset msg_disk
die:
	call print
1:	hlt
	jmp 1b

# Print the NUL-terminated string at ds:si.
print:
	lodsb
	or al, al
	jz 2f
	mov ah, 0x0e
	mov bx, 7
	int 0x10
	jmp print
2:	ret

dap1:	.byte 16, 0
	.word 15			# sectors
	.word 0x7e00, 0			# offset, segment
	.quad 1				# LBA

boot_drive: .byte 0
msg_lba:  .asciz "teenyOS: BIOS lacks LBA disk access"
msg_disk: .asciz "teenyOS: disk read error"

	.org 440			# disk signature + partition table follow
	.org 510
	.word 0xaa55

# ---------------------------------------------------------------- stage 2

	.org 512
params:
	.ascii "TEENYLDR"
kernel_lba:	.quad 0
kernel_sectors:	.long 0

stage2:
	mov si, offset msg_hello
	call print

	call pick_video

	# The first sector of the kernel says how big the setup code is.
	mov eax, [kernel_lba]
	mov [dap_lba], eax
	mov eax, [kernel_lba + 4]
	mov [dap_lba + 4], eax
	mov word ptr [dap_count], 1
	mov word ptr [dap_seg], 0x1000
	call read
	push 0x1000
	pop es
	movzx cx, byte ptr es:[0x1f1]	# setup_sects
	or cx, cx
	jnz 3f
	mov cx, 4
3:	inc cx				# plus the boot sector itself
	mov [setup_sectors], cx
	mov [dap_count], cx
	call read

	# The rest goes above 1 MB, 64 sectors at a time via a bounce buffer.
	movzx eax, word ptr [setup_sectors]
	add [dap_lba], eax
	adc dword ptr [dap_lba + 4], 0
	mov ecx, [kernel_sectors]
	sub ecx, eax
	mov [left], ecx
	mov dword ptr [dest], 0x100000
next_chunk:
	mov ecx, [left]
	or ecx, ecx
	jz loaded
	cmp ecx, 64
	jbe 4f
	mov ecx, 64
4:	mov [dap_count], cx
	mov word ptr [dap_seg], 0x2000
	call read
	# int 15h AH=87h: copy cx words from 0x20000 to [dest].
	mov eax, [dest]
	mov [gdt_dst + 2], ax
	shr eax, 16
	mov [gdt_dst + 4], al
	mov [gdt_dst + 7], ah
	movzx ecx, word ptr [dap_count]
	shl ecx, 8			# sectors * 256 words
	push ecx
	push ds
	pop es
	mov si, offset gdt
	mov ah, 0x87
	int 0x15
	pop ecx
	jc copy_error
	shl ecx, 1			# bytes
	add [dest], ecx
	movzx eax, word ptr [dap_count]
	sub [left], eax
	add [dap_lba], eax
	adc dword ptr [dap_lba + 4], 0
	mov al, '.'
	mov ah, 0x0e
	mov bx, 7
	int 0x10
	jmp next_chunk

copy_error:
	mov si, offset msg_copy
	jmp die

loaded:
	# Fill in the setup header (Documentation/arch/x86/boot.rst).
	push 0x1000
	pop es
	mov byte ptr es:[0x210], 0xff	# type_of_loader: undefined
	or byte ptr es:[0x211], 0x80	# loadflags: CAN_USE_HEAP
	mov word ptr es:[0x224], 0xde00	# heap_end_ptr
	mov dword ptr es:[0x228], 0x1e000	# cmd_line_ptr
	mov ax, [vid_mode]
	mov es:[0x1fa], ax
	# Command line at 0x1e000.
	push 0x1e00
	pop es
	xor di, di
	mov si, offset cmdline
5:	lodsb
	stosb
	or al, al
	jnz 5b

	cli
	mov ax, 0x1000
	mov ds, ax
	mov es, ax
	mov fs, ax
	mov gs, ax
	mov ss, ax
	mov sp, 0xe000
	.byte 0xea			# far jump to the setup code at 0x1020:0
	.word 0, 0x1020

# Read dap_count sectors at dap_lba to dap_seg:0.
read:
	mov si, offset dap
	mov ah, 0x42
	mov dl, [boot_drive]
	int 0x13
	jc disk_error
	ret

# Choose the largest 32 (or else 24) bit linear VESA mode up to 1280x1024
# that the kernel's own VESA probe will also accept, and store its kernel
# mode number in vid_mode. With no such mode the kernel keeps text mode.
pick_video:
	push ds
	pop es
	mov di, 0x600
	mov dword ptr [di], 0x32454256	# "VBE2"
	mov ax, 0x4f00
	int 0x10
	cmp ax, 0x004f
	jne 9f
	mov si, [0x60e]			# far pointer to the mode list
	mov ax, [0x610]
	mov fs, ax
6:	mov cx, fs:[si]
	add si, 2
	cmp cx, 0xffff
	je 9f
	test cx, 0xfe00			# the kernel ignores modes above 0x1ff
	jnz 6b
	push cx
	push si
	push fs
	push ds
	pop es
	mov di, 0x800
	mov ax, 0x4f01
	int 0x10
	pop fs
	pop si
	pop cx
	cmp ax, 0x004f
	jne 6b
	mov al, [0x800]			# attributes: supported, colour, graphics, linear
	and al, 0x99
	cmp al, 0x99
	jne 6b
	cmp byte ptr [0x818], 1		# one memory plane
	jne 6b
	cmp byte ptr [0x81b], 6		# direct colour
	jne 6b
	mov bl, [0x819]			# bits per pixel
	cmp bl, 32
	je 7f
	cmp bl, 24
	jne 6b
7:	mov ax, [0x812]			# width
	cmp ax, 1280
	ja 6b
	mov dx, [0x814]			# height
	cmp dx, 1024
	ja 6b
	# score = width * height, plus a bonus for 32 bpp
	movzx eax, ax
	movzx edx, dx
	imul eax, edx
	cmp bl, 32
	jne 8f
	add eax, 1
8:	cmp eax, [best_score]
	jbe 6b
	mov [best_score], eax
	add cx, 0x200			# VIDEO_FIRST_VESA
	mov [vid_mode], cx
	jmp 6b
9:	ret

dap:		.byte 16, 0
dap_count:	.word 0
		.word 0
dap_seg:	.word 0
dap_lba:	.quad 0

setup_sectors:	.word 0
left:		.long 0
dest:		.long 0
best_score:	.long 0
vid_mode:	.word 0xffff		# NORMAL_VGA unless a VESA mode is found

# Descriptor table for int 15h AH=87h: two null entries, source,
# destination, two more for the BIOS.
	.balign 8
gdt:	.quad 0, 0
gdt_src: .word 0xffff, 0x0000
	.byte 0x02, 0x93, 0x00, 0x00	# base 0x020000
gdt_dst: .word 0xffff, 0x0000
	.byte 0x00, 0x93, 0x00, 0x00	# base patched per chunk
	.quad 0, 0

cmdline:   .asciz "teeny.boot=bios"
msg_hello: .asciz "teenyOS "
msg_copy:  .asciz "teenyOS: high memory copy failed"

	.org 512 + 15 * 512 - 1
	.byte 0
