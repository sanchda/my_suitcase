" === Global Settings ===
set nocompatible
set encoding=utf-8"
set ignorecase
set smartcase
set foldenable
set foldmethod=indent
set t_Co=256
set t_vb=
set backspace=indent,eol,start
set incsearch
set lazyredraw
set showmatch
set showcmd
set hlsearch
set nu
set ruler
set autoread
set noerrorbells
set novisualbell
set background=dark
set laststatus=2
set updatetime=500
set tm=500
set mouse=a

set modeline

" === Custom vimrc path ===
let $MYVIMRC="$SUITCASE/vimrc"

" === What information in statusline?
set statusline+=%F

" === Syntax Highlighting & auto-indent ===
if has("nvim")
  let g:python3_host_prog = expand('~/.pyenv/versions/3.12.2/bin/python3')
lua << EOF
require'nvim-treesitter.configs'.setup {
  ensure_installed = { "c", "lua", "vim", "vimdoc", "query" },
  sync_install = false,
  auto_install = true,
  highlight = {
    enable = true,
    additional_vim_regex_highlighting = false,
  },
}
require('gitsigns').setup()
require('lspconfig').rust_analyzer.setup{}
require('lspsaga').setup()
require('lsp-inlayhints').setup()
require('telescope').setup()
vim.api.nvim_set_keymap('n', '<leader>lr', '<cmd>Telescope lsp_references<CR>', { noremap = true, silent = true })
vim.api.nvim_set_keymap('n', '<leader>ld', '<cmd>Telescope lsp_definitions<CR>', { noremap = true, silent = true })
local builtin = require('telescope.builtin')
vim.keymap.set('n', '<leader>ff', builtin.find_files, { desc = 'Telescope find files' })
vim.keymap.set('n', '<leader>fg', builtin.live_grep, { desc = 'Telescope live grep' })
vim.keymap.set('n', '<leader>fb', builtin.buffers, { desc = 'Telescope buffers' })
vim.keymap.set('n', '<leader>fh', builtin.help_tags, { desc = 'Telescope help tags' })
vim.api.nvim_set_keymap('n', '<leader>ca', '<cmd>Lspsaga code_action<CR>', { noremap = true, silent = true })
EOF

  " DiffView
  nnoremap <leader>gd :Gvdiffsplit!<CR>
  nnoremap <leader>gh :DiffviewFileHistory %<CR>
  nnoremap <leader>go :DiffviewOpen main...HEAD<CR>
  nnoremap <leader>gc :DiffviewClose<CR>
else
  syntax enable
  filetype plugin indent on
endif
set ofu=syntaxcomplete#Complete
set autoindent
set smartindent

let python_highlight_all=1

" Highlight trailing whitespace
highlight ExtraWhitespace ctermbg=red guibg=red
match ExtraWhitespace /\s\+$/

" === Color column stuff
"execute "set colorcolumn=" . join(range(81,465), ',')
highlight ColorColumn ctermbg=lightgrey

" === vimwiki/markdown ===
let g:vimwiki_list = [{'path': '~/.vimwiki/', 'syntax': 'markdown', 'ext': '.md'}]

" === Wildmenu ===
set wildmenu
set wildmode=longest,list,full
set wildignore=.svn,CVS,*.o,*.a,*.class,*.mo,*.la,*.so,*.obj,*.swp,*.jpg,*.png,*.xpm,*.gif


" === Other Settings ===
" Show < or > when characters are not displayed on the left or right.
set list
set list listchars=nbsp:¬,tab:>-,precedes:<,extends:>

" === Coding tweaks ===
fu Dave_style()
  set shiftwidth=2
  set expandtab
  set tabstop=2
  set softtabstop=2
endf

fu Corp_style()
  set shiftwidth=2
  set tabstop=2
endf

fu Bazel_style()
  set shiftwidth=4
  set expandtab
  set tabstop=4
  set softtabstop=4
endf
au BufRead,BufNewFile *.sh,*.js,*.html,*.css,*py,*pyw,*.c,*.h,*.cpp,*.hpp,*.rs call Dave_style()
au BufRead,BufNewFile *.cc,*.cxx,*.hh,*.cxx,Makefile* call Dave_style()
au BufRead,BufNewFile Makefile* set noexpandtab

" Bazel
au BufRead,BufNewFile *.bzl,*.bazel call Bazel_style()

" TEX settings
au BufRead,BufNewFile *.tex call Dave_style()

" K settings
au BufRead,BufNewFile *.k set filetype=k3
au BufRead,BufNewFile *.k call Dave_style()

" JL settings
au BufRead,BufNewFile *.jl set filetype=julia
au BufRead,BufNewFile *.jl call Dave_style()

" strace settings
au BufRead,BufNewFile *.strace set filetype=strace

" go settings
au BufRead,BufNewFile *.go call Corp_style()

" cmake settings
au BufRead,BufNewFile CMakeLists.txt,*.cmake call Dave_style()

" Use the below highlight group when displaying bad whitespace is desired.
highlight BadWhitespace ctermbg=red guibg=red
" Display tabs at the beginning of a line in Python mode as bad.
au BufRead,BufNewFile *.py,*.pyw match BadWhitespace /^\t\+/
au BufRead,BufNewFile *.k,*.py,*.pyw,*.c,*.h match BadWhitespace /\s\+$/

" Python: not needed, C: prevents insertion of '*' at the beginning of every line in a comment
au BufRead,BufNewFile *.c,*.h set formatoptions-=c formatoptions-=o formatoptions-=r

" Odin
au BufRead,BufNewFile *.odin call Dave_style()

" Neovim setup
if has("nvim")
lua << EOF
require'nvim-treesitter.configs'.setup {
  ensure_installed = { "c", "lua", "vim", "vimdoc", "query" },
  sync_install = false,
  auto_install = true,
  highlight = {
    enable = true,
    additional_vim_regex_highlighting = false,
  },
}
EOF

" DiffView
nnoremap <leader>gd :Gvdiffsplit!<CR>
nnoremap <leader>gh :DiffviewFileHistory %<CR>
nnoremap <leader>go :DiffviewOpen main...HEAD<CR>
nnoremap <leader>gc :DiffviewClose<CR>
endif
